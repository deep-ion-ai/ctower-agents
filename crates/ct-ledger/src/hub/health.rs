//! Sinais de saúde e risco do portfólio (issue #216, spec do gestor §5).
//! Calculados na leitura a partir do `hub.db`, sem tabela própria: sempre
//! atuais e ajustáveis pelos limiares de `[health]` no config do `cthubd`.
//!
//! "Atividade" de um work item = o maior entre o `ts` do último evento dele
//! (sem os de prioridade e ordem, como em `WorkItemSummary`) e o
//! `last_event_at` da última sessão. Sem nenhum dos dois, vale a criação do
//! work item.

use super::Hub;
use crate::sync::project_key;
use crate::Result;
use ct_core::{Priority, TrackerKind, WorkItemKind, WorkItemStatus};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const HOUR_MS: i64 = 3600 * 1000;
const DAY_MS: i64 = 24 * HOUR_MS;
/// Janela das sessões falhas, a mesma de `dashboard().failed_sessions`.
const FAILED_WINDOW_MS: i64 = 24 * HOUR_MS;

/// `[health]` do `config.toml` do `cthubd`. Cada sinal tem o limiar de
/// aviso (o da spec §5.2) e o de crítico (`*_critical_*`, o da tabela §5.1).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HealthThresholds {
    pub stale_days: u64,
    pub stale_critical_days: u64,
    pub waiting_hours: u64,
    pub waiting_critical_hours: u64,
    pub blocked_days: u64,
    pub blocked_critical_days: u64,
    pub paused_days: u64,
    pub pr_stale_days: u64,
    pub pr_stale_critical_days: u64,
    pub decision_review_days: u64,
    pub decision_review_critical_days: u64,
    pub failed_sessions_critical: u64,
    pub interrupted_hours: u64,
    pub interrupted_critical_hours: u64,
    pub p0_idle_hours: u64,
    pub p0_idle_critical_hours: u64,
    pub untriaged_info: u64,
    pub env_silent_hours: u64,
    pub env_silent_critical_hours: u64,
    /// Projeto Jira: último sync bem-sucedido do ambiente de sync mais velho
    /// que isto vira aviso (spec do Jira e GitLab §5.4: 3 ×
    /// `jira_sync_interval_secs`, 300 s no ambiente).
    pub tracker_sync_minutes: u64,
    pub tracker_sync_critical_hours: u64,
}

impl Default for HealthThresholds {
    fn default() -> Self {
        HealthThresholds {
            stale_days: 3,
            stale_critical_days: 7,
            waiting_hours: 24,
            waiting_critical_hours: 72,
            blocked_days: 2,
            blocked_critical_days: 7,
            paused_days: 14,
            pr_stale_days: 3,
            pr_stale_critical_days: 7,
            decision_review_days: 3,
            decision_review_critical_days: 7,
            failed_sessions_critical: 3,
            interrupted_hours: 1,
            interrupted_critical_hours: 24,
            p0_idle_hours: 24,
            p0_idle_critical_hours: 72,
            untriaged_info: 10,
            env_silent_hours: 1,
            env_silent_critical_hours: 24,
            tracker_sync_minutes: 15,
            tracker_sync_critical_hours: 24,
        }
    }
}

// A ordem das variantes é a da gravidade (`Ord`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Aviso,
    Critico,
}

impl std::str::FromStr for Severity {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "info" => Ok(Severity::Info),
            "aviso" => Ok(Severity::Aviso),
            "critico" => Ok(Severity::Critico),
            _ => Err(format!("severidade inválida: {s} (info, aviso ou critico)")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum HealthKind {
    Parado,
    EsperandoVoce,
    Bloqueado,
    PausadoLongo,
    PrSemAndamento,
    DecisoesSemRevisao,
    SessoesFalhas,
    SessaoInterrompida,
    P0SemAndamento,
    EpicoSemSessao,
    NaoTriado,
    AmbienteMudo,
    GithubSemSync,
    /// Projeto Jira sem ambiente de sync, com o sync parado ou em erro
    /// (spec do Jira e GitLab §5.4).
    RastreadorSemSync,
}

/// Um sinal. Projeto, número e título faltam quando o sinal não é de um
/// work item (`ambiente_mudo` não tem projeto; `nao_triado`,
/// `github_sem_sync` e `rastreador_sem_sync` não têm número).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HealthSignal {
    pub kind: HealthKind,
    pub severity: Severity,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub number: Option<i64>,
    /// Referência pronta do work item (`ct_core::item_ref`), com `number`.
    #[serde(rename = "ref", default)]
    #[cfg_attr(feature = "ts", ts(rename = "ref"))]
    pub item_ref: Option<String>,
    pub title: Option<String>,
    pub priority: Option<Priority>,
    /// Desde quando a condição vale (ms), quando se aplica.
    pub since: Option<i64>,
    /// Medida atual, legível (ex.: "8 dias", "3 sessões").
    pub value: String,
    /// Limiar ultrapassado, legível (ex.: "7 dias").
    pub threshold: String,
    pub detail: Option<String>,
    /// Ambiente do sinal (`ambiente_mudo`, sessões).
    pub env: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HealthCounts {
    pub critico: usize,
    pub aviso: usize,
    pub info: usize,
}

/// Resposta de `hub.health.get`. `signals` vem por gravidade, prioridade
/// (`P0` primeiro, não triado no fim) e idade (mais velho primeiro).
/// `counts` conta os sinais devolvidos.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubHealth {
    pub generated_at: i64,
    pub signals: Vec<HealthSignal>,
    pub counts: HealthCounts,
}

/// Filtros de `Hub::health`.
#[derive(Debug, Clone)]
pub struct HealthFilter {
    /// `owner/repo`; `None` = todos (com os sinais de ambiente).
    pub project: Option<(String, String)>,
    pub min_severity: Severity,
}

impl Default for HealthFilter {
    fn default() -> Self {
        HealthFilter {
            project: None,
            min_severity: Severity::Info,
        }
    }
}

/// Idade legível: dias a partir de 48 h, horas a partir de 1 h, senão
/// minutos.
pub fn fmt_age(ms: i64) -> String {
    let ms = ms.max(0);
    if ms >= 2 * DAY_MS {
        format!("{} dias", ms / DAY_MS)
    } else if ms >= HOUR_MS {
        format!("{} h", ms / HOUR_MS)
    } else {
        format!("{} min", ms / 60_000)
    }
}

/// Gravidade de uma idade: crítico a partir de `crit`, aviso a partir de
/// `warn`. Devolve o limiar ultrapassado.
fn by_age(age: i64, warn: i64, crit: i64) -> Option<(Severity, i64)> {
    if age >= crit {
        Some((Severity::Critico, crit))
    } else if age >= warn {
        Some((Severity::Aviso, warn))
    } else {
        None
    }
}

fn days(d: u64) -> i64 {
    d as i64 * DAY_MS
}

fn hours(h: u64) -> i64 {
    h as i64 * HOUR_MS
}

/// Work item com o que os critérios leem.
struct Item {
    id: i64,
    owner: String,
    repo: String,
    number: i64,
    item_ref: String,
    kind: WorkItemKind,
    status: WorkItemStatus,
    priority: Option<Priority>,
    priority_at: Option<i64>,
    created_at: i64,
    title: Option<String>,
    issue_open: bool,
    /// Forja do projeto: `PR #n` ou `MR !n` nas mensagens.
    forge: ct_core::ForgeKind,
}

impl Item {
    fn signal(&self, kind: HealthKind, severity: Severity) -> HealthSignal {
        HealthSignal {
            kind,
            severity,
            owner: Some(self.owner.clone()),
            repo: Some(self.repo.clone()),
            number: Some(self.number),
            item_ref: Some(self.item_ref.clone()),
            title: self.title.clone(),
            priority: self.priority,
            since: None,
            value: String::new(),
            threshold: String::new(),
            detail: None,
            env: None,
        }
    }
}

impl Hub {
    /// Sinais de saúde do portfólio (ou de um projeto), com os limiares
    /// `t`. `gh_sync_interval_secs` = intervalo do sync do GitHub do hub
    /// (`0` = desligado, sem o sinal `github_sem_sync`).
    pub fn health(
        &self,
        t: &HealthThresholds,
        gh_sync_interval_secs: u64,
        f: &HealthFilter,
        now: i64,
    ) -> Result<HubHealth> {
        let key = f.project.as_ref().map(|(o, r)| project_key(o, r));
        let mut out = Vec::new();
        let items = self.health_items(key.as_deref())?;
        let by_id: HashMap<i64, &Item> = items.iter().map(|i| (i.id, i)).collect();
        for it in &items {
            self.item_signals(t, it, now, &mut out)?;
        }
        self.epic_signals(t, &items, now, &mut out)?;
        self.session_signals(t, &by_id, now, &mut out)?;
        self.project_signals(t, gh_sync_interval_secs, key.as_deref(), now, &mut out)?;
        if key.is_none() {
            self.env_signals(t, now, &mut out)?;
        }
        out.retain(|s| s.severity >= f.min_severity);
        out.sort_by(|a, b| {
            b.severity
                .cmp(&a.severity)
                .then_with(|| match (a.priority, b.priority) {
                    (Some(x), Some(y)) => (x as u8).cmp(&(y as u8)),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                })
                .then_with(|| {
                    a.since
                        .unwrap_or(i64::MAX)
                        .cmp(&b.since.unwrap_or(i64::MAX))
                })
                .then_with(|| (&a.owner, &a.repo, a.number).cmp(&(&b.owner, &b.repo, b.number)))
        });
        let mut counts = HealthCounts::default();
        for s in &out {
            match s.severity {
                Severity::Critico => counts.critico += 1,
                Severity::Aviso => counts.aviso += 1,
                Severity::Info => counts.info += 1,
            }
        }
        Ok(HubHealth {
            generated_at: now,
            signals: out,
            counts,
        })
    }

    /// Work items dos projetos cadastrados (sem o `#0` do projeto).
    fn health_items(&self, key: Option<&str>) -> Result<Vec<Item>> {
        let mut stmt = self.conn.prepare(
            "SELECT w.id, p.gh_owner, p.gh_repo, w.gh_number, w.kind, w.status, w.priority,
                    w.priority_at, w.created_at, i.title, coalesce(i.state, 'open') = 'open',
                    p.tracker_kind, p.tracker_config, p.forge_kind
             FROM work_item w
             JOIN project p ON p.id = w.project_id AND p.deleted_at IS NULL AND p.kind = 'github'
             LEFT JOIN issue_cache i ON i.project_id = w.project_id AND i.gh_number = w.gh_number
             WHERE w.gh_number > 0 AND (?1 IS NULL OR p.key = ?1)
             ORDER BY p.key, w.gh_number",
        )?;
        let rows = stmt.query_map([key], |r| {
            let (owner, repo, number): (String, String, i64) = (r.get(1)?, r.get(2)?, r.get(3)?);
            let tracker = crate::tracker_col(r, 11)?;
            Ok(Item {
                id: r.get(0)?,
                item_ref: ct_core::item_ref(
                    &ct_core::TrackerRef::new(&owner, &repo, &tracker),
                    number,
                ),
                owner,
                repo,
                number,
                kind: crate::parse_col(r, 4)?,
                status: crate::parse_col(r, 5)?,
                priority: crate::opt_parse_col(r, 6)?,
                priority_at: r.get(7)?,
                created_at: r.get(8)?,
                title: r.get(9)?,
                issue_open: r.get(10)?,
                forge: crate::parse_col(r, 13)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Última atividade dos work items `ids` (ver o topo do módulo).
    fn activity(&self, ids: &[i64]) -> Result<Option<i64>> {
        let list = ids
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        let t: i64 = self.conn.query_row(
            &format!(
                "SELECT max(
                   coalesce((SELECT max(ts) FROM event WHERE work_item_id IN ({list})
                               AND type NOT IN ('priority_set', 'rank_moved')), 0),
                   coalesce((SELECT max(last_event_at) FROM session
                             WHERE work_item_id IN ({list})), 0))"
            ),
            [],
            |r| r.get(0),
        )?;
        Ok((t > 0).then_some(t))
    }

    fn has_live_session(&self, ids: &[i64]) -> Result<bool> {
        let list = ids
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        Ok(self.conn.query_row(
            &format!(
                "SELECT EXISTS (SELECT 1 FROM session WHERE work_item_id IN ({list})
                                AND state IN ('starting', 'running', 'idle'))"
            ),
            [],
            |r| r.get(0),
        )?)
    }

    /// `ts` e motivo do evento de status que vale (o mais novo por HLC).
    fn status_since(&self, id: i64) -> Result<Option<(i64, Option<String>)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT ts, json_extract(payload, '$.reason') FROM event
                 WHERE work_item_id = ?1 AND type IN ('status_changed', 'status_set_manual')
                 ORDER BY hlc DESC, env_id DESC LIMIT 1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    }

    fn item_signals(
        &self,
        t: &HealthThresholds,
        it: &Item,
        now: i64,
        out: &mut Vec<HealthSignal>,
    ) -> Result<()> {
        use WorkItemStatus::*;
        let done = it.status == Concluido || !it.issue_open;
        let activity = self.activity(&[it.id])?.unwrap_or(it.created_at);
        let live = self.has_live_session(&[it.id])?;

        if matches!(it.status, EmExecucao | EmRevisao) && !live && it.issue_open {
            let age = now - activity;
            if let Some((sev, th)) = by_age(age, days(t.stale_days), days(t.stale_critical_days)) {
                let mut s = it.signal(HealthKind::Parado, sev);
                s.since = Some(activity);
                s.value = fmt_age(age);
                s.threshold = fmt_age(th);
                s.detail = Some(format!("{}, sem sessão viva", it.status));
                out.push(s);
            }
        }

        if !done {
            let timed = match it.status {
                AguardandoVoce => Some((
                    HealthKind::EsperandoVoce,
                    hours(t.waiting_hours),
                    hours(t.waiting_critical_hours),
                )),
                Bloqueado => Some((
                    HealthKind::Bloqueado,
                    days(t.blocked_days),
                    days(t.blocked_critical_days),
                )),
                // Só informativo: sem nível crítico.
                Pausado => Some((HealthKind::PausadoLongo, days(t.paused_days), i64::MAX)),
                _ => None,
            };
            if let Some((kind, warn, crit)) = timed {
                let (since, reason) = self.status_since(it.id)?.unwrap_or((it.created_at, None));
                let age = now - since;
                if let Some((sev, th)) = by_age(age, warn, crit) {
                    let sev = if kind == HealthKind::PausadoLongo {
                        Severity::Info
                    } else {
                        sev
                    };
                    let mut s = it.signal(kind, sev);
                    s.since = Some(since);
                    s.value = fmt_age(age);
                    s.threshold = fmt_age(th);
                    s.detail = reason.map(|r| format!("{}: {r}", it.status));
                    out.push(s);
                }
            }
        }

        // PRs (MRs) abertos sem andamento na forja.
        let prs: Vec<(i64, i64)> = {
            let mut stmt = self.conn.prepare(
                "SELECT number, updated_at FROM pr_cache
                 WHERE work_item_id = ?1 AND state = 'open' ORDER BY number",
            )?;
            let rows = stmt.query_map([it.id], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (pr, updated) in prs {
            let age = now - updated;
            if let Some((sev, th)) =
                by_age(age, days(t.pr_stale_days), days(t.pr_stale_critical_days))
            {
                let mut s = it.signal(HealthKind::PrSemAndamento, sev);
                s.since = Some(updated);
                s.value = fmt_age(age);
                s.threshold = fmt_age(th);
                s.detail = Some(format!(
                    "{} aberto sem atualização",
                    ct_core::change_ref(it.forge, pr)
                ));
                out.push(s);
            }
        }

        // Decisões autônomas sem revisão.
        let (pending, oldest): (i64, Option<i64>) = self.conn.query_row(
            "SELECT count(*), min(created_at) FROM decision
             WHERE work_item_id = ?1 AND origin = 'autonoma' AND reviewed_at IS NULL",
            [it.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if let Some(oldest) = oldest {
            let age = now - oldest;
            let hit = if it.status == Concluido {
                Some((Severity::Critico, None))
            } else {
                by_age(
                    age,
                    days(t.decision_review_days),
                    days(t.decision_review_critical_days),
                )
                .map(|(s, th)| (s, Some(th)))
            };
            if let Some((sev, th)) = hit {
                let mut s = it.signal(HealthKind::DecisoesSemRevisao, sev);
                s.since = Some(oldest);
                s.value = format!(
                    "{pending} {}, a mais velha há {}",
                    if pending == 1 {
                        "decisão"
                    } else {
                        "decisões"
                    },
                    fmt_age(age)
                );
                s.threshold = match th {
                    Some(th) => fmt_age(th),
                    None => "concluído com pendência".into(),
                };
                out.push(s);
            }
        }

        // P0 parado: conta desde a última atividade ou desde que virou P0.
        // Sub-issue que aguarda o épico não está parada: espera o PR dele.
        if it.priority == Some(Priority::P0) && !done && !live && it.status != AguardandoEpico {
            let since = activity.max(it.priority_at.unwrap_or(0));
            let age = now - since;
            if let Some((sev, th)) =
                by_age(age, hours(t.p0_idle_hours), hours(t.p0_idle_critical_hours))
            {
                let mut s = it.signal(HealthKind::P0SemAndamento, sev);
                s.since = Some(since);
                s.value = fmt_age(age);
                s.threshold = fmt_age(th);
                s.detail = Some(format!("P0 em {}, sem sessão viva", it.status));
                out.push(s);
            }
        }
        Ok(())
    }

    /// Épicos já começados, com sub-issues abertas no espelho e nenhuma
    /// sessão viva no épico nem nas sub-issues, parados há `stale_days`.
    fn epic_signals(
        &self,
        t: &HealthThresholds,
        items: &[Item],
        now: i64,
        out: &mut Vec<HealthSignal>,
    ) -> Result<()> {
        use WorkItemStatus::*;
        for it in items.iter().filter(|i| {
            i.kind == WorkItemKind::Epic
                && i.issue_open
                && !matches!(i.status, Backlog | Concluido | Pausado)
        }) {
            // Sub-issue aberta que já aguarda o épico não pede sessão.
            let open_children: i64 = self.conn.query_row(
                "SELECT count(*) FROM issue_cache i
                 LEFT JOIN work_item w ON w.project_id = i.project_id AND w.gh_number = i.gh_number
                 WHERE i.project_id = (SELECT project_id FROM work_item WHERE id = ?1)
                   AND i.parent_number = ?2 AND i.state = 'open'
                   AND coalesce(w.status, '') <> 'aguardando_epico'",
                params![it.id, it.number],
                |r| r.get(0),
            )?;
            if open_children == 0 {
                continue;
            }
            let ids: Vec<i64> = self.epic_work_items(it.id)?.iter().map(|w| w.id).collect();
            if self.has_live_session(&ids)? {
                continue;
            }
            let activity = self.activity(&ids)?.unwrap_or(it.created_at);
            let age = now - activity;
            if let Some((sev, th)) = by_age(age, days(t.stale_days), days(t.stale_critical_days)) {
                let mut s = it.signal(HealthKind::EpicoSemSessao, sev);
                s.since = Some(activity);
                s.value = fmt_age(age);
                s.threshold = fmt_age(th);
                s.detail = Some(format!(
                    "{open_children} sub-issue(s) aberta(s), nenhuma sessão viva"
                ));
                out.push(s);
            }
        }
        Ok(())
    }

    /// Sessões falhas (por work item, na janela de 24 h) e interrompidas.
    fn session_signals(
        &self,
        t: &HealthThresholds,
        items: &HashMap<i64, &Item>,
        now: i64,
        out: &mut Vec<HealthSignal>,
    ) -> Result<()> {
        let names = self.env_names()?;
        let failed: Vec<(i64, i64, i64)> = {
            let mut stmt = self.conn.prepare(
                "SELECT work_item_id, count(*), min(ended_at) FROM session
                 WHERE state = 'exited' AND exit_code IS NOT NULL AND exit_code <> 0
                   AND ended_at >= ?1
                 GROUP BY work_item_id ORDER BY work_item_id",
            )?;
            let rows = stmt.query_map([now - FAILED_WINDOW_MS], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (wi, n, first) in failed {
            let Some(it) = items.get(&wi) else { continue };
            let crit = t.failed_sessions_critical.max(1) as i64;
            let (sev, th) = if n >= crit {
                (Severity::Critico, crit)
            } else {
                (Severity::Aviso, 1)
            };
            let mut s = it.signal(HealthKind::SessoesFalhas, sev);
            s.since = Some(first);
            s.value = format!("{n} {} em 24 h", if n == 1 { "sessão" } else { "sessões" });
            s.threshold = format!("{th} em 24 h");
            out.push(s);
        }
        let interrupted: Vec<(String, i64, String, i64)> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, work_item_id, env_id, coalesce(ended_at, last_event_at) FROM session
                 WHERE state = 'interrupted' ORDER BY started_at",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (sid, wi, env, since) in interrupted {
            let Some(it) = items.get(&wi) else { continue };
            let age = now - since;
            if let Some((sev, th)) = by_age(
                age,
                hours(t.interrupted_hours),
                hours(t.interrupted_critical_hours),
            ) {
                let mut s = it.signal(HealthKind::SessaoInterrompida, sev);
                s.since = Some(since);
                s.value = fmt_age(age);
                s.threshold = fmt_age(th);
                s.detail = Some(format!("sessão {sid} com recuperação pendente"));
                s.env = Some(names.get(&env).cloned().unwrap_or(env));
                out.push(s);
            }
        }
        Ok(())
    }

    /// Por projeto: issues não triadas e sync do GitHub atrasado.
    fn project_signals(
        &self,
        t: &HealthThresholds,
        gh_sync_interval_secs: u64,
        key: Option<&str>,
        now: i64,
        out: &mut Vec<HealthSignal>,
    ) -> Result<()> {
        for p in self.list_projects()? {
            if key.is_some_and(|k| k != project_key(&p.gh_owner, &p.gh_repo)) {
                continue;
            }
            let signal = |kind, severity| HealthSignal {
                kind,
                severity,
                owner: Some(p.gh_owner.clone()),
                repo: Some(p.gh_repo.clone()),
                number: None,
                item_ref: None,
                title: Some(p.name.clone()),
                priority: None,
                since: None,
                value: String::new(),
                threshold: String::new(),
                detail: None,
                env: None,
            };
            let untriaged: i64 = self.conn.query_row(
                "SELECT count(*) FROM issue_cache i
                 LEFT JOIN work_item w ON w.project_id = i.project_id AND w.gh_number = i.gh_number
                 WHERE i.project_id = ?1 AND i.state = 'open' AND w.priority IS NULL
                   AND coalesce(w.status, 'backlog') <> 'concluido'",
                [p.id],
                |r| r.get(0),
            )?;
            if t.untriaged_info > 0 && untriaged >= t.untriaged_info as i64 {
                let mut s = signal(HealthKind::NaoTriado, Severity::Info);
                s.value = format!("{untriaged} issues");
                s.threshold = format!("{} issues", t.untriaged_info);
                out.push(s);
            }
            if p.tracker.kind() == TrackerKind::Jira {
                if let Some(s) = self.tracker_signal(t, &p, now, signal)? {
                    out.push(s);
                }
                continue;
            }
            if gh_sync_interval_secs == 0 {
                continue;
            }
            let limit = 3 * gh_sync_interval_secs as i64 * 1000;
            let st = self.sync_state(p.id)?;
            let last_ok = st.as_ref().and_then(|s| s.last_ok_at);
            let error = st.as_ref().and_then(|s| s.last_error.clone());
            let since = last_ok.unwrap_or(p.created_at);
            let late = now - since >= limit;
            if late || error.is_some() {
                let mut s = signal(HealthKind::GithubSemSync, Severity::Aviso);
                s.since = Some(since);
                s.value = match last_ok {
                    Some(ok) => format!("último sync há {}", fmt_age(now - ok)),
                    None => "nunca sincronizado".into(),
                };
                s.threshold = fmt_age(limit);
                s.detail = error;
                out.push(s);
            }
        }
        Ok(())
    }

    /// `rastreador_sem_sync` do projeto Jira: sem ambiente de sync, último
    /// sync bem-sucedido velho (aviso a partir de `tracker_sync_minutes`,
    /// crítico a partir de `tracker_sync_critical_hours`) ou último ciclo com
    /// erro. O detalhe cita o ambiente e o erro.
    fn tracker_signal(
        &self,
        t: &HealthThresholds,
        p: &super::HubProject,
        now: i64,
        signal: impl Fn(HealthKind, Severity) -> HealthSignal,
    ) -> Result<Option<HealthSignal>> {
        let st = self.sync_state(p.id)?;
        let last_ok = st.as_ref().and_then(|s| s.last_ok_at);
        let error = st.as_ref().and_then(|s| s.last_error.clone());
        let since = last_ok.unwrap_or(p.created_at);
        let age = now - since;
        let warn = t.tracker_sync_minutes as i64 * 60_000;
        let crit = hours(t.tracker_sync_critical_hours);
        let env: Option<(String, Option<i64>)> = match &p.sync_env_id {
            Some(id) => self
                .conn
                .query_row(
                    "SELECT name, last_seen_at FROM env WHERE env_id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .or_else(|| Some((id.clone(), None))),
            None => None,
        };
        let by_age = by_age(age, warn, crit);
        let (severity, threshold) = match (&env, &by_age, &error) {
            (_, Some((sev, th)), _) => (*sev, *th),
            // Sem ambiente de sync ou com erro, mesmo recente: aviso.
            (None, None, _) | (_, None, Some(_)) => (Severity::Aviso, warn),
            (Some(_), None, None) => return Ok(None),
        };
        let mut s = signal(HealthKind::RastreadorSemSync, severity);
        s.since = Some(since);
        s.value = match last_ok {
            Some(ok) => format!("último sync há {}", fmt_age(now - ok)),
            None => "nunca sincronizado".into(),
        };
        s.threshold = fmt_age(threshold);
        let mut detail = match &env {
            None => "sem ambiente de sync: atribua o projeto a um ambiente que alcance o Jira e o \
                     GitLab, ou escolha o ambiente de sync em Hub › Projetos"
                .to_string(),
            Some((name, seen)) => {
                let mut d = format!("ambiente de sync {name}");
                match seen {
                    None => d.push_str(", que ainda não falou com o hub"),
                    Some(seen) if now - seen >= warn => {
                        d.push_str(&format!(", sem falar com o hub há {}", fmt_age(now - seen)))
                    }
                    Some(_) => {}
                }
                d
            }
        };
        if let Some(e) = &error {
            detail.push_str(&format!(": {e}"));
        }
        s.detail = Some(detail);
        s.env = env.map(|(name, _)| name);
        Ok(Some(s))
    }

    /// Ambientes com pendências no outbox que pararam de enviar.
    fn env_signals(
        &self,
        t: &HealthThresholds,
        now: i64,
        out: &mut Vec<HealthSignal>,
    ) -> Result<()> {
        for e in self.list_envs()? {
            let Some(seen) = e.last_seen_at.filter(|_| e.pending > 0) else {
                continue;
            };
            let age = now - seen;
            if let Some((sev, th)) = by_age(
                age,
                hours(t.env_silent_hours),
                hours(t.env_silent_critical_hours),
            ) {
                out.push(HealthSignal {
                    kind: HealthKind::AmbienteMudo,
                    severity: sev,
                    owner: None,
                    repo: None,
                    number: None,
                    item_ref: None,
                    title: None,
                    priority: None,
                    since: Some(seen),
                    value: fmt_age(age),
                    threshold: fmt_age(th),
                    detail: Some(format!("{} mudança(s) pendente(s) no outbox", e.pending)),
                    env: Some(e.name),
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GithubSnapshot, IssueRow, NewHubProject};
    use HealthKind::*;
    use Severity::*;

    const NOW: i64 = 1_000 * DAY_MS;

    struct H {
        hub: Hub,
        pid: i64,
        seq: i64,
    }

    fn issue(n: i64, parent: Option<i64>, subs: bool, state: &str) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("Issue {n}"),
            state: state.into(),
            labels: vec![],
            parent_number: parent,
            has_sub_issues: subs,
            url: "u".into(),
            updated_at: 1,
            sprints: Vec::new(),
        }
    }

    impl H {
        fn new(issues: Vec<IssueRow>) -> H {
            let hub = Hub::open_in_memory().unwrap();
            hub.add_project(&NewHubProject {
                owner: "o".into(),
                repo: "a".into(),
                name: "a".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
            let pid = hub.find_project("o", "a").unwrap().unwrap().id;
            hub.apply_github(
                &GithubSnapshot {
                    project_id: pid,
                    issues,
                    prs: vec![],
                    etags: vec![],
                    synced_at: 1,
                },
                "hub",
            )
            .unwrap();
            hub.conn
                .execute(
                    "INSERT INTO env (env_id, name, created_at, last_seen_at, pending)
                     VALUES ('e1', 'wsl', 0, ?1, 0)",
                    [NOW],
                )
                .unwrap();
            // Sync do GitHub em dia (os testes de sync mexem nele).
            hub.conn
                .execute(
                    "INSERT OR REPLACE INTO sync_state (project_id, last_attempt_at, last_ok_at)
                     VALUES (?1, ?2, ?2)",
                    params![pid, NOW],
                )
                .unwrap();
            H { hub, pid, seq: 0 }
        }

        /// Work item criado em `created` (ms antes de `NOW`).
        fn item(&self, n: i64, kind: &str, status: &str, created: i64) -> i64 {
            self.hub
                .conn
                .execute(
                    "INSERT INTO work_item (project_id, gh_number, kind, status, created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![self.pid, n, kind, status, NOW - created],
                )
                .unwrap();
            self.hub.conn.last_insert_rowid()
        }

        fn event(&mut self, wi: i64, kind: &str, ago: i64, payload: &str) {
            self.seq += 1;
            self.hub
                .conn
                .execute(
                    "INSERT INTO event (uid, env_id, hlc, ts, project_id, work_item_id, type, payload)
                     VALUES (?1, 'e1', ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![format!("u{}", self.seq), self.seq, NOW - ago, self.pid, wi, kind, payload],
                )
                .unwrap();
        }

        fn session(&mut self, wi: i64, state: &str, exit: Option<i32>, ago: i64) {
            self.seq += 1;
            self.hub
                .conn
                .execute(
                    "INSERT INTO session (id, env_id, work_item_id, tool, cwd, state, exit_code,
                                          started_at, last_event_at, ended_at)
                     VALUES (?1, 'e1', ?2, 'claude', '/w', ?3, ?4, ?5, ?5, ?6)",
                    params![
                        format!("s{}", self.seq),
                        wi,
                        state,
                        exit,
                        NOW - ago,
                        (state == "exited" || state == "interrupted").then_some(NOW - ago)
                    ],
                )
                .unwrap();
        }

        fn decision(&mut self, wi: i64, origin: &str, ago: i64, reviewed: bool) {
            self.seq += 1;
            self.hub
                .conn
                .execute(
                    "INSERT INTO decision (uid, env_id, work_item_id, origin, title, context,
                                           decision, reviewed_at, created_at)
                     VALUES (?1, 'e1', ?2, ?3, 't', 'c', 'd', ?4, ?5)",
                    params![
                        format!("d{}", self.seq),
                        wi,
                        origin,
                        reviewed.then_some(NOW),
                        NOW - ago
                    ],
                )
                .unwrap();
        }

        fn health(&self) -> HubHealth {
            self.health_with(&HealthThresholds::default(), 0, &HealthFilter::default())
        }

        fn health_with(&self, t: &HealthThresholds, gh: u64, f: &HealthFilter) -> HubHealth {
            self.hub.health(t, gh, f, NOW).unwrap()
        }
    }

    /// (kind, severity, número) de cada sinal, na ordem devolvida.
    fn sig(h: &HubHealth) -> Vec<(HealthKind, Severity, Option<i64>)> {
        h.signals
            .iter()
            .map(|s| (s.kind, s.severity, s.number))
            .collect()
    }

    fn of(h: &HubHealth, kind: HealthKind) -> Vec<(Severity, Option<i64>)> {
        h.signals
            .iter()
            .filter(|s| s.kind == kind)
            .map(|s| (s.severity, s.number))
            .collect()
    }

    fn open(ns: &[i64]) -> Vec<IssueRow> {
        ns.iter().map(|&n| issue(n, None, false, "open")).collect()
    }

    #[test]
    fn parado_pelos_limiares_e_sessao_viva_cancela() {
        let mut h = H::new(open(&[1, 2, 3, 4, 5]));
        let a = h.item(1, "issue", "em_execucao", 30 * DAY_MS);
        h.event(a, "status_changed", 2 * DAY_MS, "{}"); // abaixo do limiar
        let b = h.item(2, "issue", "em_revisao", 30 * DAY_MS);
        h.event(b, "status_changed", 4 * DAY_MS, "{}"); // aviso
        let c = h.item(3, "issue", "em_execucao", 30 * DAY_MS);
        h.session(c, "exited", Some(0), 8 * DAY_MS); // crítico (atividade = sessão)
        let d = h.item(4, "issue", "em_execucao", 30 * DAY_MS);
        h.session(d, "running", None, 9 * DAY_MS); // sessão viva: não é parado
        let e = h.item(5, "issue", "em_execucao", 30 * DAY_MS);
        h.event(e, "status_changed", 9 * DAY_MS, "{}");
        h.event(e, "priority_set", HOUR_MS, "{}"); // triar não é atividade
        let r = h.health();
        assert_eq!(
            of(&r, Parado),
            vec![(Critico, Some(5)), (Critico, Some(3)), (Aviso, Some(2))]
        );
        let s = r.signals.iter().find(|s| s.number == Some(2)).unwrap();
        assert_eq!(
            (s.value.as_str(), s.threshold.as_str()),
            ("4 dias", "3 dias")
        );
        assert_eq!(s.since, Some(NOW - 4 * DAY_MS));
        assert_eq!(s.owner.as_deref(), Some("o"));
        assert_eq!(s.title.as_deref(), Some("Issue 2"));
        // Limiar configurável.
        let t = HealthThresholds {
            stale_days: 1,
            stale_critical_days: 100,
            ..Default::default()
        };
        let r = h.health_with(&t, 0, &HealthFilter::default());
        assert_eq!(of(&r, Parado).len(), 4);
        assert!(of(&r, Parado).iter().all(|(s, _)| *s == Aviso));
    }

    #[test]
    fn status_que_esperam_alguem_contam_desde_o_evento_de_status() {
        let mut h = H::new(open(&[1, 2, 3, 4, 5, 6]));
        let a = h.item(1, "issue", "aguardando_voce", DAY_MS * 10);
        h.event(a, "status_changed", 25 * HOUR_MS, "{}");
        let b = h.item(2, "issue", "aguardando_voce", DAY_MS * 10);
        h.event(b, "status_changed", 73 * HOUR_MS, "{}");
        let c = h.item(3, "issue", "bloqueado", DAY_MS * 10);
        h.event(c, "status_changed", 9 * DAY_MS, "{}");
        // O evento de status mais novo (por HLC) é o que vale.
        h.event(
            c,
            "status_set_manual",
            3 * DAY_MS,
            r#"{"to":"bloqueado","reason":"revisão externa"}"#,
        );
        let d = h.item(4, "issue", "bloqueado", DAY_MS * 10);
        h.event(d, "status_set_manual", 8 * DAY_MS, "{}");
        let e = h.item(5, "issue", "pausado", DAY_MS * 30);
        h.event(e, "status_set_manual", 15 * DAY_MS, "{}");
        let f = h.item(6, "issue", "pausado", DAY_MS * 30);
        h.event(f, "status_set_manual", 13 * DAY_MS, "{}");
        let r = h.health();
        assert_eq!(
            of(&r, EsperandoVoce),
            vec![(Critico, Some(2)), (Aviso, Some(1))]
        );
        assert_eq!(
            of(&r, Bloqueado),
            vec![(Critico, Some(4)), (Aviso, Some(3))]
        );
        assert_eq!(of(&r, PausadoLongo), vec![(Info, Some(5))]);
        let s = r
            .signals
            .iter()
            .find(|s| s.kind == Bloqueado && s.number == Some(3))
            .unwrap();
        assert_eq!(s.detail.as_deref(), Some("bloqueado: revisão externa"));
        assert_eq!(s.since, Some(NOW - 3 * DAY_MS));
    }

    #[test]
    fn prs_decisoes_e_sessoes() {
        let mut h = H::new(open(&[1, 2, 3, 4]));
        let a = h.item(1, "issue", "em_revisao", DAY_MS);
        h.session(a, "running", None, 0);
        for (n, ago, state) in [
            (10, 4 * DAY_MS, "open"),
            (11, 8 * DAY_MS, "open"),
            (12, 30 * DAY_MS, "merged"),
            (13, DAY_MS, "open"),
        ] {
            h.hub
                .conn
                .execute(
                    "INSERT INTO pr_cache (project_id, number, work_item_id, head, base, state,
                                           url, updated_at, synced_at)
                     VALUES (?1, ?2, ?3, 'h', 'b', ?4, 'u', ?5, 0)",
                    params![h.pid, n, a, state, NOW - ago],
                )
                .unwrap();
        }
        let b = h.item(2, "issue", "em_execucao", DAY_MS);
        h.session(b, "running", None, 0);
        h.decision(b, "autonoma", 4 * DAY_MS, false);
        h.decision(b, "autonoma", 30 * DAY_MS, true); // revisada
        h.decision(b, "orientada", 30 * DAY_MS, false); // não é autônoma
        h.decision(b, "autonoma", DAY_MS, false);
        let c = h.item(3, "issue", "concluido", DAY_MS);
        h.decision(c, "autonoma", HOUR_MS, false); // concluído com pendência
        let d = h.item(4, "issue", "backlog", DAY_MS);
        h.session(d, "exited", Some(1), HOUR_MS);
        h.session(d, "exited", Some(1), 2 * HOUR_MS);
        h.session(d, "exited", Some(0), HOUR_MS); // saiu bem
        h.session(d, "exited", Some(1), 30 * HOUR_MS); // fora da janela
        h.session(c, "exited", Some(2), HOUR_MS);
        h.session(c, "exited", Some(2), HOUR_MS);
        h.session(c, "exited", Some(2), HOUR_MS);
        h.session(a, "interrupted", None, 30 * 60_000); // abaixo de 1 h
        h.session(b, "interrupted", None, 2 * HOUR_MS);
        h.session(d, "interrupted", None, 25 * HOUR_MS);
        let r = h.health();
        assert_eq!(
            of(&r, PrSemAndamento),
            vec![(Critico, Some(1)), (Aviso, Some(1))]
        );
        let pr = r.signals.iter().find(|s| s.kind == PrSemAndamento).unwrap();
        assert_eq!(pr.detail.as_deref(), Some("PR #11 aberto sem atualização"));
        assert_eq!(
            of(&r, DecisoesSemRevisao),
            vec![(Critico, Some(3)), (Aviso, Some(2))]
        );
        let dec = r
            .signals
            .iter()
            .find(|s| s.kind == DecisoesSemRevisao && s.number == Some(2))
            .unwrap();
        assert_eq!(dec.value, "2 decisões, a mais velha há 4 dias");
        assert_eq!(
            of(&r, SessoesFalhas),
            vec![(Critico, Some(3)), (Aviso, Some(4))]
        );
        assert_eq!(
            of(&r, SessaoInterrompida),
            vec![(Critico, Some(4)), (Aviso, Some(2))]
        );
        let int = r
            .signals
            .iter()
            .find(|s| s.kind == SessaoInterrompida)
            .unwrap();
        assert_eq!(int.env.as_deref(), Some("wsl"));
        // Sessão viva cancela o parado.
        assert!(of(&r, Parado).is_empty());
    }

    #[test]
    fn p0_sem_andamento_conta_desde_que_virou_p0() {
        let h = H::new(open(&[1, 2, 3]));
        let a = h.item(1, "issue", "backlog", 30 * DAY_MS);
        let b = h.item(2, "issue", "backlog", 30 * DAY_MS);
        let c = h.item(3, "issue", "backlog", 30 * DAY_MS);
        for (wi, set_ago) in [(a, 2 * HOUR_MS), (b, 30 * HOUR_MS), (c, 4 * DAY_MS)] {
            h.hub
                .conn
                .execute(
                    "UPDATE work_item SET priority = 'P0', rank = 1, priority_at = ?2 WHERE id = ?1",
                    params![wi, NOW - set_ago],
                )
                .unwrap();
        }
        let r = h.health();
        assert_eq!(
            of(&r, P0SemAndamento),
            vec![(Critico, Some(3)), (Aviso, Some(2))]
        );
        assert!(r.signals.iter().all(|s| s.priority == Some(Priority::P0)));
    }

    #[test]
    fn aguardando_epico_nao_e_parado_nem_atencao() {
        let mut h = H::new(vec![
            issue(10, None, true, "open"),
            issue(11, Some(10), false, "open"),
            issue(12, Some(10), false, "open"),
        ]);
        h.item(10, "epic", "em_execucao", 30 * DAY_MS);
        let a = h.item(11, "issue", "aguardando_epico", 30 * DAY_MS);
        h.event(a, "status_changed", 9 * DAY_MS, "{}");
        let b = h.item(12, "issue", "aguardando_epico", 30 * DAY_MS);
        h.event(b, "status_changed", 9 * DAY_MS, "{}");
        h.hub
            .conn
            .execute(
                "UPDATE work_item SET priority = 'P0', rank = 1, priority_at = ?2 WHERE id = ?1",
                params![a, NOW - 4 * DAY_MS],
            )
            .unwrap();
        let r = h.health();
        // A sub-issue não é parada nem P0 sem andamento, e o épico, cujas
        // sub-issues abertas só aguardam o merge dele, não pede sessão.
        let sinais: Vec<_> = r.signals.iter().map(|s| (s.kind, s.number)).collect();
        assert!(
            sinais.iter().all(|(_, n)| !matches!(n, Some(11 | 12))),
            "{sinais:?}"
        );
        assert!(of(&r, EpicoSemSessao).is_empty());
        // Com uma sub-issue aberta que não aguarda o épico, o sinal volta.
        h.hub
            .conn
            .execute(
                "UPDATE work_item SET status = 'em_execucao' WHERE id = ?1",
                [b],
            )
            .unwrap();
        assert_eq!(of(&h.health(), EpicoSemSessao), vec![(Critico, Some(10))]);
    }

    #[test]
    fn epico_com_sub_issues_abertas_sem_sessao() {
        let mut h = H::new(vec![
            issue(10, None, true, "open"),
            issue(11, Some(10), false, "open"),
            issue(12, Some(10), false, "closed"),
            issue(20, None, true, "open"),
            issue(21, Some(20), false, "open"),
            issue(30, None, true, "open"),
            issue(31, Some(30), false, "closed"),
        ]);
        let e = h.item(10, "epic", "em_execucao", 30 * DAY_MS);
        let f = h.item(11, "issue", "backlog", 30 * DAY_MS);
        h.event(f, "status_changed", 4 * DAY_MS, "{}");
        let e2 = h.item(20, "epic", "em_execucao", 30 * DAY_MS);
        let f2 = h.item(21, "issue", "em_execucao", 30 * DAY_MS);
        h.session(f2, "running", None, 9 * DAY_MS); // sessão viva na filha
                                                    // Sem sub-issue aberta: fica de fora.
        h.item(30, "epic", "em_execucao", 30 * DAY_MS);
        let _ = (e, e2);
        let r = h.health();
        assert_eq!(of(&r, EpicoSemSessao), vec![(Aviso, Some(10))]);
        let s = r.signals.iter().find(|s| s.kind == EpicoSemSessao).unwrap();
        assert_eq!(s.since, Some(NOW - 4 * DAY_MS));
    }

    #[test]
    fn nao_triado_github_e_ambiente_mudo() {
        let ns: Vec<i64> = (1..=12).collect();
        let mut issues = open(&ns);
        issues.push(issue(13, None, false, "closed"));
        let h = H::new(issues);
        h.hub
            .set_priority("o", "a", 1, Some(Priority::P2), None, None, "hub")
            .unwrap();
        let r = h.health();
        assert_eq!(of(&r, NaoTriado), vec![(Info, None)]);
        let s = r.signals.iter().find(|s| s.kind == NaoTriado).unwrap();
        assert_eq!(s.value, "11 issues");
        assert!(of(&r, GithubSemSync).is_empty(), "sync desligado");

        // Sync a cada 60 s: atrasado depois de 3 min, ou com erro.
        let r = h.health_with(&HealthThresholds::default(), 60, &HealthFilter::default());
        assert!(of(&r, GithubSemSync).is_empty());
        h.hub
            .conn
            .execute(
                "UPDATE sync_state SET last_ok_at = ?1 WHERE project_id = ?2",
                params![NOW - 4 * 60_000, h.pid],
            )
            .unwrap();
        let r = h.health_with(&HealthThresholds::default(), 60, &HealthFilter::default());
        assert_eq!(of(&r, GithubSemSync), vec![(Aviso, None)]);
        h.hub
            .conn
            .execute(
                "UPDATE sync_state SET last_ok_at = ?1, last_error = 'HTTP 401' WHERE project_id = ?2",
                params![NOW, h.pid],
            )
            .unwrap();
        let r = h.health_with(&HealthThresholds::default(), 60, &HealthFilter::default());
        let s = r.signals.iter().find(|s| s.kind == GithubSemSync).unwrap();
        assert_eq!(s.detail.as_deref(), Some("HTTP 401"));

        for (id, seen, pending) in [
            ("e2", 2 * HOUR_MS, 5),
            ("e3", 25 * HOUR_MS, 1),
            ("e4", 30 * HOUR_MS, 0),
        ] {
            h.hub
                .conn
                .execute(
                    "INSERT INTO env (env_id, name, created_at, last_seen_at, pending)
                     VALUES (?1, ?1, 0, ?2, ?3)",
                    params![id, NOW - seen, pending],
                )
                .unwrap();
        }
        let r = h.health();
        let mudos: Vec<_> = r
            .signals
            .iter()
            .filter(|s| s.kind == AmbienteMudo)
            .map(|s| (s.severity, s.env.clone().unwrap()))
            .collect();
        assert_eq!(mudos, vec![(Critico, "e3".into()), (Aviso, "e2".into())]);
        // Filtrar por projeto deixa de fora os sinais de ambiente.
        let f = HealthFilter {
            project: Some(("O".into(), "A".into())),
            min_severity: Info,
        };
        assert!(of(
            &h.health_with(&HealthThresholds::default(), 0, &f),
            AmbienteMudo
        )
        .is_empty());
        let f = HealthFilter {
            project: Some(("o".into(), "x".into())),
            min_severity: Info,
        };
        assert!(h
            .health_with(&HealthThresholds::default(), 0, &f)
            .signals
            .is_empty());
    }

    #[test]
    fn ordem_contagem_e_gravidade_minima() {
        let mut h = H::new(open(&[1, 2, 3, 4, 5]));
        let a = h.item(1, "issue", "bloqueado", 30 * DAY_MS);
        h.event(a, "status_set_manual", 3 * DAY_MS, "{}"); // aviso, não triado
        let b = h.item(2, "issue", "bloqueado", 30 * DAY_MS);
        h.event(b, "status_set_manual", 4 * DAY_MS, "{}"); // aviso, P1
        let c = h.item(3, "issue", "bloqueado", 30 * DAY_MS);
        h.event(c, "status_set_manual", 10 * DAY_MS, "{}"); // crítico
        let d = h.item(4, "issue", "pausado", 30 * DAY_MS);
        h.event(d, "status_set_manual", 20 * DAY_MS, "{}"); // info
        let e = h.item(5, "issue", "bloqueado", 30 * DAY_MS);
        h.event(e, "status_set_manual", 2 * DAY_MS + HOUR_MS, "{}"); // aviso, P1, mais novo
        h.hub
            .conn
            .execute(
                "UPDATE work_item SET priority = 'P1' WHERE id IN (?1, ?2)",
                params![b, e],
            )
            .unwrap();
        let r = h.health();
        assert_eq!(
            sig(&r),
            vec![
                (Bloqueado, Critico, Some(3)),
                (Bloqueado, Aviso, Some(2)),
                (Bloqueado, Aviso, Some(5)),
                (Bloqueado, Aviso, Some(1)),
                (PausadoLongo, Info, Some(4)),
            ]
        );
        assert_eq!(
            r.counts,
            HealthCounts {
                critico: 1,
                aviso: 3,
                info: 1
            }
        );
        assert_eq!(r.generated_at, NOW);
        let f = HealthFilter {
            project: None,
            min_severity: Aviso,
        };
        let r = h.health_with(&HealthThresholds::default(), 0, &f);
        assert_eq!(r.signals.len(), 4);
        assert_eq!(r.counts.info, 0);
        assert_eq!("critico".parse::<Severity>(), Ok(Critico));
        assert!("grave".parse::<Severity>().is_err());
    }

    #[test]
    fn idade_legivel() {
        assert_eq!(fmt_age(30 * 60_000), "30 min");
        assert_eq!(fmt_age(25 * HOUR_MS), "25 h");
        assert_eq!(fmt_age(8 * DAY_MS + HOUR_MS), "8 dias");
    }
}
