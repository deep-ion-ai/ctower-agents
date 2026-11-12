//! A decisão do agendador do piloto automático (#183), pura: recebe a
//! fotografia do hub num instante ([`Snapshot`]) e devolve o que fazer
//! ([`Outcome`]): estacionar, retomar ou iniciar sessões, as recusas a
//! registrar e a fila de cada projeto (para o painel, #185). Sem I/O nem
//! relógio: os testes montam a fotografia à mão.
//!
//! # Regras
//!
//! - **Pausado:** nenhum início. Estacionar e retomar continuam: a sessão
//!   em andamento termina a issue.
//! - **Ordem:** projetos na ordem do hub (`project.rank`, a mesma da tela
//!   de projetos e do ranking), e dentro do projeto por prioridade, depois
//!   pela ordem do item na classe e pelo número.
//! - **Item elegível:** issue (não épico) no espelho, em `backlog`, com o
//!   rótulo de aprovação do projeto (rótulo desligado = nenhum aprovado),
//!   com prioridade, com modelo e sem sessão viva.
//! - **Projeto:** modo autônomo ligado, ambiente do modo online, conta do
//!   ambiente com política que permite agora, branch de integração sem
//!   conflito nem erro, ambiente com vaga (padrão: uma sessão autônoma por
//!   vez) e projeto com vaga no limite de trabalho em andamento.
//! - **Sonda sem leitura:** conta sem nenhuma leitura de uma janela (o
//!   orçamento recusa só por `no_reading`) e sem sessão autônoma viva pode
//!   iniciar uma sessão, a sonda (`StartItem::probe`): é o statusline dela
//!   que traz a primeira leitura. Com uma sessão viva na conta, espera a
//!   leitura dela; no máximo uma sonda por vez.
//! - **Uma por conta por ciclo:** a decisão do orçamento é de antes do
//!   início; o ciclo seguinte reavalia com a leitura nova.
//! - **Limite real:** conta com a janela de 5h ou a semanal em 100% (ou
//!   mais) antes do reset estaciona as sessões ociosas dos ambientes
//!   autônomos dela; depois do `resets_at` (mais [`RESUME_MARGIN_MS`]) o
//!   agendador as retoma, antes de iniciar qualquer outra na conta.

use ct_core::plan::budget::{BlockReason, BudgetDecision};
use ct_core::plan::{PlanView, WindowView};
use ct_core::{Priority, SessionState, WipUsage, WorkItemKind, WorkItemStatus};
use ct_ledger::{AutopilotSession, AutopilotSettings, ParkedSession};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Folga depois do `resets_at` antes de retomar: a renovação não é exata.
pub const RESUME_MARGIN_MS: i64 = 60_000;

/// Fotografia do hub para uma decisão.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub now: i64,
    pub settings: AutopilotSettings,
    /// Projetos com o modo autônomo ligado, na ordem do hub.
    pub projects: Vec<ProjectSnap>,
    pub envs: BTreeMap<String, EnvSnap>,
    /// Por chave da conta.
    pub accounts: BTreeMap<String, AccountSnap>,
    /// Sessões de implementação vivas (mais as estacionadas e os inícios
    /// que o ambiente ainda não sincronizou, como `starting`).
    pub sessions: Vec<AutopilotSession>,
    pub parked: Vec<ParkedSession>,
}

#[derive(Debug, Clone, Default)]
pub struct EnvSnap {
    pub name: String,
    pub online: bool,
    /// Marcado como autônomo (#181).
    pub autonomous: bool,
    /// Chave da conta do Claude Code do ambiente; `None` = desconhecida.
    pub account: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AccountSnap {
    /// `None` = conta sem política (piloto desligado nela).
    pub decision: Option<BudgetDecision>,
    /// Janela esgotada agora (limite real do plano).
    pub exhausted: Option<Exhausted>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Exhausted {
    /// `five_hour` ou `seven_day`.
    pub window: String,
    pub resets_at: i64,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectSnap {
    pub owner: String,
    pub repo: String,
    /// Ambiente do modo.
    pub env_id: String,
    /// Mensagem do relato com a branch de integração em conflito.
    pub conflict: Option<String>,
    /// Mensagem do relato `error` da branch de integração (ex.: o push
    /// recusado).
    pub autonomy_error: Option<String>,
    pub wip: WipUsage,
    /// Rótulo de aprovação ativo do projeto; `None` = desligado.
    pub approval_label: Option<String>,
    /// Na ordem do projeto (prioridade, ordem na classe, número).
    pub items: Vec<ItemSnap>,
}

#[derive(Debug, Clone)]
pub struct ItemSnap {
    pub number: i64,
    /// `None` = fora do espelho.
    pub title: Option<String>,
    pub kind: WorkItemKind,
    pub status: WorkItemStatus,
    pub priority: Option<Priority>,
    pub model: Option<String>,
    pub approved: bool,
}

/// Janela esgotada da visão do plano (a de reset mais tarde, se as duas).
pub fn exhausted(plan: &PlanView) -> Option<Exhausted> {
    [("five_hour", plan.five_hour), ("seven_day", plan.seven_day)]
        .into_iter()
        .filter_map(|(name, w)| match w {
            WindowView::Ok {
                used_percentage,
                resets_at,
                ..
            } if used_percentage >= 100.0 => Some(Exhausted {
                window: name.into(),
                resets_at,
            }),
            _ => None,
        })
        .max_by_key(|e| e.resets_at)
}

/// Como retomar a sessão estacionada.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ResumeVia {
    /// Viva: `session.continue` (texto fixo no PTY).
    Continue,
    /// Encerrada: `session.resume` (`claude --resume`).
    Resume,
}

/// Uma ação do ciclo.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Park(ParkedSession),
    /// Tira do estacionamento sem retomar (a sessão sumiu ou voltou a rodar).
    Unpark {
        parked: ParkedSession,
        message: String,
    },
    Resume {
        parked: ParkedSession,
        via: ResumeVia,
    },
    Start(StartItem),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StartItem {
    pub account: String,
    pub env_id: String,
    pub owner: String,
    pub repo: String,
    pub number: i64,
    pub model: String,
    /// Sessão sonda: a conta não tem leitura do plano (veja o módulo).
    pub probe: bool,
}

/// Recusa relevante, gravada como evento `refused` (só quando o motivo
/// muda no escopo).
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub scope: String,
    pub reason: String,
    pub message: String,
    pub account: Option<String>,
    pub env_id: Option<String>,
    pub owner: Option<String>,
    pub repo: Option<String>,
}

/// Item da fila de um projeto, com o motivo de ter ficado de fora.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct QueueItem {
    pub number: i64,
    pub title: Option<String>,
    pub priority: Option<Priority>,
    pub model: Option<String>,
    /// `None` = elegível. Senão: `epic`, `status`, `not_in_mirror`,
    /// `not_approved`, `no_priority`, `no_model` ou `live_session`.
    pub skip: Option<String>,
}

/// Situação de um projeto autônomo no ciclo.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ProjectPlan {
    pub owner: String,
    pub repo: String,
    pub env_id: String,
    pub env_name: Option<String>,
    pub env_online: bool,
    /// Conta do ambiente (some na web).
    pub account: Option<String>,
    /// Por que nada inicia agora; `None` = pode iniciar (ou iniciou).
    /// `paused`, `env_offline`, `unknown_account`, `no_policy`, `parked`,
    /// `budget:<motivos>`, `conflict`, `autonomy_error`, `env_busy`,
    /// `wip_limit`, `account_turn` (a conta já iniciou neste ciclo) ou
    /// `no_eligible`.
    pub blocked: Option<String>,
    /// Texto do motivo, para a pessoa.
    pub blocked_message: Option<String>,
    /// Item que inicia agora.
    pub next: Option<i64>,
    /// O início é a sonda sem leitura do plano (`next` sai mesmo com o
    /// orçamento em `no_reading`).
    pub probe: bool,
    pub queue: Vec<QueueItem>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    pub actions: Vec<Action>,
    pub refusals: Vec<Refusal>,
    pub projects: Vec<ProjectPlan>,
}

/// Código estável dos motivos do orçamento: `budget:band_closed+after_cutoff`.
pub fn budget_reason(reasons: &[BlockReason]) -> String {
    let codes: BTreeSet<&str> = reasons
        .iter()
        .map(|r| match r {
            BlockReason::BandClosed { .. } => "band_closed",
            BlockReason::NoBand => "no_band",
            BlockReason::AfterCutoff => "after_cutoff",
            BlockReason::FiveHourOverCap { .. } => "five_hour_over_cap",
            BlockReason::WeeklyOverLimit { .. } => "weekly_over_limit",
            BlockReason::NoReading { .. } => "no_reading",
        })
        .collect();
    format!("budget:{}", codes.into_iter().collect::<Vec<_>>().join("+"))
}

fn skip_of(item: &ItemSnap, label: Option<&str>, live: bool) -> Option<&'static str> {
    if item.kind != WorkItemKind::Issue {
        Some("epic")
    } else if item.status != WorkItemStatus::Backlog {
        Some("status")
    } else if item.title.is_none() {
        Some("not_in_mirror")
    } else if label.is_none() || !item.approved {
        Some("not_approved")
    } else if item.priority.is_none() {
        Some("no_priority")
    } else if item.model.is_none() {
        Some("no_model")
    } else if live {
        Some("live_session")
    } else {
        None
    }
}

/// Decide o ciclo.
pub fn decide(s: &Snapshot) -> Outcome {
    let mut out = Outcome::default();
    let now = s.now;
    let env = |id: &str| s.envs.get(id);
    let account_of = |env_id: &str| env(env_id).and_then(|e| e.account.clone());

    // 1. Estacionadas: sumiram, voltaram a rodar, renovaram ou seguem.
    let parked_ids: BTreeSet<&str> = s.parked.iter().map(|p| p.session_id.as_str()).collect();
    let mut accounts_waiting: BTreeSet<String> = BTreeSet::new();
    for p in &s.parked {
        let sess = s.sessions.iter().find(|x| x.session_id == p.session_id);
        let state = sess.map(|x| x.state);
        match state {
            None | Some(SessionState::Archived) => {
                out.actions.push(Action::Unpark {
                    parked: p.clone(),
                    message: format!(
                        "{}/{}#{}: a sessão estacionada não existe mais",
                        p.owner, p.repo, p.gh_number
                    ),
                });
                continue;
            }
            Some(SessionState::Running) if now < p.resume_at => {
                out.actions.push(Action::Unpark {
                    parked: p.clone(),
                    message: format!(
                        "{}/{}#{}: a sessão voltou a rodar antes do reset",
                        p.owner, p.repo, p.gh_number
                    ),
                });
                continue;
            }
            _ => {}
        }
        accounts_waiting.insert(p.account_key.clone());
        if now < p.resume_at {
            continue;
        }
        // Renovou, mas a leitura nova diz que outra janela esgotou.
        if let Some(ex) = s
            .accounts
            .get(&p.account_key)
            .and_then(|a| a.exhausted.as_ref())
            .filter(|ex| ex.resets_at > now)
        {
            let resume_at = ex.resets_at + RESUME_MARGIN_MS;
            if resume_at != p.resume_at {
                out.actions.push(Action::Park(ParkedSession {
                    window: ex.window.clone(),
                    resume_at,
                    parked_at: now,
                    ..p.clone()
                }));
            }
            continue;
        }
        if !env(&p.env_id).is_some_and(|e| e.online) {
            continue;
        }
        let via = match state {
            Some(st) if st.is_live() => ResumeVia::Continue,
            _ => ResumeVia::Resume,
        };
        out.actions.push(Action::Resume {
            parked: p.clone(),
            via,
        });
    }

    // 2. Estacionar: sessões ociosas de ambientes autônomos numa conta com
    // política e com a janela esgotada.
    for x in &s.sessions {
        if parked_ids.contains(x.session_id.as_str()) || x.state != SessionState::Idle {
            continue;
        }
        let Some(e) = env(&x.env_id).filter(|e| e.autonomous) else {
            continue;
        };
        let Some(key) = &e.account else { continue };
        let Some(a) = s.accounts.get(key).filter(|a| a.decision.is_some()) else {
            continue;
        };
        let Some(ex) = a.exhausted.as_ref().filter(|ex| ex.resets_at > now) else {
            continue;
        };
        accounts_waiting.insert(key.clone());
        out.actions.push(Action::Park(ParkedSession {
            session_id: x.session_id.clone(),
            env_id: x.env_id.clone(),
            account_key: key.clone(),
            owner: x.owner.clone(),
            repo: x.repo.clone(),
            gh_number: x.gh_number,
            window: ex.window.clone(),
            resume_at: ex.resets_at + RESUME_MARGIN_MS,
            parked_at: now,
        }));
    }

    // 3. Inícios.
    let live_on_env = |env_id: &str| {
        s.sessions
            .iter()
            .filter(|x| x.env_id == env_id && x.state.is_live())
            .count() as u32
    };
    let live_item = |owner: &str, repo: &str, n: i64| {
        s.sessions.iter().any(|x| {
            x.state.is_live()
                && x.gh_number == n
                && x.owner.eq_ignore_ascii_case(owner)
                && x.repo.eq_ignore_ascii_case(repo)
        })
    };
    // Sessões vivas nos ambientes autônomos de cada conta (a sonda só
    // inicia sem nenhuma).
    let live_on_account = |key: &str| {
        s.sessions.iter().any(|x| {
            x.state.is_live()
                && env(&x.env_id).is_some_and(|e| e.autonomous && e.account.as_deref() == Some(key))
        })
    };
    let mut started_envs: BTreeMap<String, u32> = BTreeMap::new();
    let mut started_accounts: BTreeSet<String> = BTreeSet::new();
    for p in &s.projects {
        let slug = format!("{}/{}", p.owner, p.repo);
        let e = env(&p.env_id);
        let account = account_of(&p.env_id);
        let queue: Vec<QueueItem> = p
            .items
            .iter()
            .map(|i| QueueItem {
                number: i.number,
                title: i.title.clone(),
                priority: i.priority,
                model: i.model.clone(),
                skip: skip_of(
                    i,
                    p.approval_label.as_deref(),
                    live_item(&p.owner, &p.repo, i.number),
                )
                .map(String::from),
            })
            .collect();
        let mut plan = ProjectPlan {
            owner: p.owner.clone(),
            repo: p.repo.clone(),
            env_id: p.env_id.clone(),
            env_name: e.map(|e| e.name.clone()),
            env_online: e.is_some_and(|e| e.online),
            account: account.clone(),
            blocked: None,
            blocked_message: None,
            next: None,
            probe: false,
            queue,
        };
        let project_refusal = |reason: &str, message: String| Refusal {
            scope: format!("project:{slug}"),
            reason: reason.into(),
            message,
            account: account.clone(),
            env_id: Some(p.env_id.clone()),
            owner: Some(p.owner.clone()),
            repo: Some(p.repo.clone()),
        };
        let mut probe = false;
        let blocked = (|| -> Option<(String, String, Option<Refusal>)> {
            if s.settings.paused {
                return Some(("paused".into(), "piloto automático pausado".into(), None));
            }
            if !plan.env_online {
                let m = format!(
                    "{slug}: o ambiente autônomo {} está offline",
                    plan.env_name.as_deref().unwrap_or(&p.env_id)
                );
                return Some((
                    "env_offline".into(),
                    m.clone(),
                    Some(project_refusal("env_offline", m)),
                ));
            }
            let Some(key) = &account else {
                let m = format!(
                    "{slug}: a conta do Claude Code do ambiente autônomo é desconhecida (sem \
                     leitura do plano)"
                );
                return Some((
                    "unknown_account".into(),
                    m.clone(),
                    Some(project_refusal("unknown_account", m)),
                ));
            };
            let acc_refusal = |reason: &str, message: String| Refusal {
                scope: format!("account:{key}"),
                reason: reason.into(),
                message,
                account: Some(key.clone()),
                env_id: None,
                owner: None,
                repo: None,
            };
            let Some(decision) = s.accounts.get(key).and_then(|a| a.decision.as_ref()) else {
                let m = "conta sem política de orçamento: o piloto automático está desligado nela"
                    .to_string();
                return Some((
                    "no_policy".into(),
                    m.clone(),
                    Some(acc_refusal("no_policy", m)),
                ));
            };
            if accounts_waiting.contains(key) {
                return Some((
                    "parked".into(),
                    "há sessão estacionada pelo limite do plano na conta: ela é retomada antes \
                     de qualquer início"
                        .into(),
                    None,
                ));
            }
            // Só falta a leitura e a conta não tem sessão autônoma viva: a
            // sonda inicia (as outras condições abaixo continuam valendo).
            probe = decision.only_no_reading() && !live_on_account(key);
            if !decision.can_start && !probe {
                let reason = budget_reason(&decision.reasons);
                let mut m = format!(
                    "a política do plano não permite iniciar agora: {}",
                    decision
                        .reasons
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join("; ")
                );
                if decision.only_no_reading() {
                    m.push_str(
                        " (a sessão autônoma em andamento na conta traz a leitura; a sonda \
                         espera por ela)",
                    );
                }
                return Some((reason.clone(), m.clone(), Some(acc_refusal(&reason, m))));
            }
            if let Some(c) = &p.conflict {
                let m = format!("{slug}: {c}; nenhuma issue autônoma começa até resolver");
                return Some((
                    "conflict".into(),
                    m.clone(),
                    Some(project_refusal("conflict", m)),
                ));
            }
            if let Some(err) = &p.autonomy_error {
                let m = format!(
                    "{slug}: a branch de integração falhou ({err}); nenhuma issue autônoma \
                     começa até resolver e alinhar de novo"
                );
                return Some((
                    "autonomy_error".into(),
                    m.clone(),
                    Some(project_refusal("autonomy_error", m)),
                ));
            }
            let busy = live_on_env(&p.env_id) + started_envs.get(&p.env_id).copied().unwrap_or(0);
            if busy >= s.settings.max_sessions_per_env {
                return Some((
                    "env_busy".into(),
                    format!(
                        "o ambiente autônomo já roda {busy} de {} sessão(ões) autônoma(s)",
                        s.settings.max_sessions_per_env
                    ),
                    None,
                ));
            }
            if let Some(r) = p.wip.reason() {
                let m = format!("{slug}: {r}");
                return Some((
                    "wip_limit".into(),
                    m.clone(),
                    Some(project_refusal("wip_limit", m)),
                ));
            }
            if started_accounts.contains(key) {
                return Some((
                    "account_turn".into(),
                    "a conta já iniciou uma sessão neste ciclo".into(),
                    None,
                ));
            }
            None
        })();
        match blocked {
            Some((code, message, refusal)) => {
                plan.blocked = Some(code);
                plan.blocked_message = Some(message);
                out.refusals.extend(refusal);
            }
            None => {
                let next = p
                    .items
                    .iter()
                    .zip(&plan.queue)
                    .find(|(_, q)| q.skip.is_none())
                    .map(|(i, _)| i);
                match next {
                    Some(i) => {
                        let key = account.clone().expect("conferida acima");
                        plan.next = Some(i.number);
                        plan.probe = probe;
                        *started_envs.entry(p.env_id.clone()).or_default() += 1;
                        started_accounts.insert(key.clone());
                        out.actions.push(Action::Start(StartItem {
                            account: key,
                            env_id: p.env_id.clone(),
                            owner: p.owner.clone(),
                            repo: p.repo.clone(),
                            number: i.number,
                            model: i.model.clone().expect("elegível tem modelo"),
                            probe,
                        }));
                    }
                    None => {
                        let m = format!(
                            "{slug}: nenhuma issue elegível (aprovada, com prioridade, com \
                             modelo e sem sessão viva)"
                        );
                        plan.blocked = Some("no_eligible".into());
                        plan.blocked_message = Some(m.clone());
                        out.refusals.push(project_refusal("no_eligible", m));
                    }
                }
            }
        }
        out.projects.push(plan);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_core::plan::budget::{evaluate, BudgetPolicy};
    use ct_core::WipLimits;

    const H: i64 = 3_600_000;
    /// Sábado, 2026-11-14 12:00 em GMT-3 (15:00 UTC): fim de semana, aberto.
    const SAB: i64 = 1_794_668_400_000;
    /// Terça, 2026-11-10 05:00 em GMT-3: depois do corte de início (04:00).
    const TER_CORTE: i64 = 1_794_297_600_000;

    fn plan(five: f64, week: f64, now: i64) -> PlanView {
        PlanView {
            five_hour: WindowView::Ok {
                used_percentage: five,
                resets_at: now + 2 * H,
                read_at: now - 60_000,
            },
            seven_day: WindowView::Ok {
                used_percentage: week,
                resets_at: now + 3 * 24 * H,
                read_at: now - 60_000,
            },
            read_at: Some(now - 60_000),
        }
    }

    fn account(five: f64, week: f64, now: i64) -> AccountSnap {
        let v = plan(five, week, now);
        AccountSnap {
            decision: Some(evaluate(&BudgetPolicy::default(), &v, now)),
            exhausted: exhausted(&v),
        }
    }

    fn item(n: i64, p: Option<Priority>, model: Option<&str>, approved: bool) -> ItemSnap {
        ItemSnap {
            number: n,
            title: Some(format!("ACME-{n}")),
            kind: WorkItemKind::Issue,
            status: WorkItemStatus::Backlog,
            priority: p,
            model: model.map(String::from),
            approved,
        }
    }

    fn project(repo: &str, env: &str, items: Vec<ItemSnap>) -> ProjectSnap {
        ProjectSnap {
            owner: "acme".into(),
            repo: repo.into(),
            env_id: env.into(),
            conflict: None,
            autonomy_error: None,
            wip: WipUsage::default(),
            approval_label: Some("aprovada".into()),
            items,
        }
    }

    fn snap(now: i64) -> Snapshot {
        let mut s = Snapshot {
            now,
            ..Default::default()
        };
        s.envs.insert(
            "env-auto".into(),
            EnvSnap {
                name: "autonomo".into(),
                online: true,
                autonomous: true,
                account: Some("conta-a".into()),
            },
        );
        s.accounts
            .insert("conta-a".into(), account(10.0, 10.0, now));
        s.projects.push(project(
            "acme-web",
            "env-auto",
            vec![
                item(123, Some(Priority::P1), Some("opus"), true),
                item(124, Some(Priority::P0), None, true),
            ],
        ));
        s
    }

    fn starts(o: &Outcome) -> Vec<(String, i64, String)> {
        o.actions
            .iter()
            .filter_map(|a| match a {
                Action::Start(i) => Some((i.repo.clone(), i.number, i.model.clone())),
                _ => None,
            })
            .collect()
    }

    fn session(id: &str, env: &str, n: i64, state: SessionState) -> AutopilotSession {
        AutopilotSession {
            session_id: id.into(),
            env_id: env.into(),
            state,
            owner: "acme".into(),
            repo: "acme-web".into(),
            gh_number: n,
            started_at: 1,
        }
    }

    #[test]
    fn inicia_o_item_elegivel_com_o_modelo_da_issue() {
        let o = decide(&snap(SAB));
        assert_eq!(starts(&o), vec![("acme-web".into(), 123, "opus".into())]);
        assert!(o.refusals.is_empty(), "{:?}", o.refusals);
        let q = &o.projects[0].queue;
        assert_eq!(q[0].skip, None);
        assert_eq!(q[1].skip.as_deref(), Some("no_model"));
        assert_eq!(o.projects[0].next, Some(123));
    }

    #[test]
    fn sem_rotulo_sem_prioridade_ou_sem_modelo_nunca_inicia() {
        let mut s = snap(SAB);
        s.projects[0].items = vec![
            item(1, Some(Priority::P0), Some("opus"), false),
            item(2, None, Some("opus"), true),
            item(3, Some(Priority::P0), None, true),
        ];
        let o = decide(&s);
        assert!(starts(&o).is_empty());
        let skips: Vec<_> = o.projects[0]
            .queue
            .iter()
            .map(|q| q.skip.clone().unwrap())
            .collect();
        assert_eq!(skips, ["not_approved", "no_priority", "no_model"]);
        assert_eq!(o.projects[0].blocked.as_deref(), Some("no_eligible"));
        assert_eq!(o.refusals[0].reason, "no_eligible");
        // Projeto com o rótulo desligado: nenhum item conta como aprovado.
        s.projects[0].items = vec![item(1, Some(Priority::P0), Some("opus"), true)];
        s.projects[0].approval_label = None;
        let o = decide(&s);
        assert!(starts(&o).is_empty());
        assert_eq!(o.projects[0].queue[0].skip.as_deref(), Some("not_approved"));
        // Épico, item fora do backlog e fora do espelho também ficam de fora.
        let mut epic = item(5, Some(Priority::P0), Some("opus"), true);
        epic.kind = WorkItemKind::Epic;
        let mut doing = item(6, Some(Priority::P0), Some("opus"), true);
        doing.status = WorkItemStatus::EmRevisao;
        let mut fora = item(7, Some(Priority::P0), Some("opus"), true);
        fora.title = None;
        s.projects[0].approval_label = Some("aprovada".into());
        s.projects[0].items = vec![epic, doing, fora];
        let o = decide(&s);
        assert!(starts(&o).is_empty());
        let skips: Vec<_> = o.projects[0]
            .queue
            .iter()
            .map(|q| q.skip.clone().unwrap())
            .collect();
        assert_eq!(skips, ["epic", "status", "not_in_mirror"]);
    }

    #[test]
    fn depois_do_corte_nao_inicia_e_a_sessao_em_andamento_continua() {
        let mut s = snap(TER_CORTE);
        s.accounts
            .insert("conta-a".into(), account(10.0, 10.0, TER_CORTE));
        s.sessions
            .push(session("s-viva", "env-auto", 99, SessionState::Running));
        s.settings.max_sessions_per_env = 2;
        let o = decide(&s);
        assert!(starts(&o).is_empty());
        // Nenhuma ação sobre a sessão viva (o agendador nunca encerra).
        assert!(o.actions.is_empty(), "{:?}", o.actions);
        assert_eq!(
            o.projects[0].blocked.as_deref(),
            Some("budget:after_cutoff")
        );
        assert_eq!(o.refusals[0].scope, "account:conta-a");
        assert!(o.refusals[0].message.contains("corte de início"));
    }

    #[test]
    fn pausado_nao_inicia_nada() {
        let mut s = snap(SAB);
        s.settings.paused = true;
        let o = decide(&s);
        assert!(o.actions.is_empty());
        assert!(o.refusals.is_empty());
        assert_eq!(o.projects[0].blocked.as_deref(), Some("paused"));
        // A fila continua visível para o painel.
        assert_eq!(o.projects[0].queue.len(), 2);
    }

    #[test]
    fn uma_sessao_autonoma_por_ambiente_por_padrao() {
        let mut s = snap(SAB);
        s.sessions
            .push(session("s1", "env-auto", 99, SessionState::Idle));
        let o = decide(&s);
        assert!(starts(&o).is_empty());
        assert_eq!(o.projects[0].blocked.as_deref(), Some("env_busy"));
        assert!(o.refusals.is_empty());
        s.settings.max_sessions_per_env = 2;
        assert_eq!(starts(&decide(&s)).len(), 1);
        // Dois projetos no mesmo ambiente: só um inicia no ciclo.
        let mut s = snap(SAB);
        s.projects.push(project(
            "acme-api",
            "env-auto",
            vec![item(7, Some(Priority::P0), Some("sonnet"), true)],
        ));
        let o = decide(&s);
        assert_eq!(starts(&o), vec![("acme-web".into(), 123, "opus".into())]);
        assert_eq!(o.projects[1].blocked.as_deref(), Some("env_busy"));
    }

    #[test]
    fn ordem_entre_projetos_e_depois_por_prioridade() {
        let mut s = snap(SAB);
        s.settings.max_sessions_per_env = 5;
        s.envs.insert(
            "env-2".into(),
            EnvSnap {
                name: "autonomo-2".into(),
                online: true,
                autonomous: true,
                account: Some("conta-b".into()),
            },
        );
        s.accounts
            .insert("conta-b".into(), account(10.0, 10.0, SAB));
        // Primeiro projeto só com P3; o segundo tem P0, mas vem depois.
        s.projects[0].items = vec![item(30, Some(Priority::P3), Some("haiku"), true)];
        s.projects.push(project(
            "acme-api",
            "env-auto",
            vec![item(1, Some(Priority::P0), Some("opus"), true)],
        ));
        s.projects.push(project(
            "acme-app",
            "env-2",
            vec![item(2, Some(Priority::P0), Some("opus"), true)],
        ));
        let o = decide(&s);
        // Uma por conta por ciclo: acme-web na conta-a, acme-app na conta-b.
        assert_eq!(
            starts(&o),
            vec![
                ("acme-web".into(), 30, "haiku".into()),
                ("acme-app".into(), 2, "opus".into())
            ]
        );
        assert_eq!(o.projects[1].blocked.as_deref(), Some("account_turn"));
    }

    #[test]
    fn recusas_de_ambiente_conta_conflito_e_limite() {
        let mut s = snap(SAB);
        s.envs.get_mut("env-auto").unwrap().online = false;
        let o = decide(&s);
        assert_eq!(o.refusals[0].reason, "env_offline");
        assert_eq!(o.refusals[0].scope, "project:acme/acme-web");

        let mut s = snap(SAB);
        s.envs.get_mut("env-auto").unwrap().account = None;
        assert_eq!(decide(&s).refusals[0].reason, "unknown_account");

        let mut s = snap(SAB);
        s.accounts.get_mut("conta-a").unwrap().decision = None;
        let o = decide(&s);
        assert_eq!(o.refusals[0].reason, "no_policy");
        assert!(starts(&o).is_empty());

        let mut s = snap(SAB);
        s.projects[0].conflict = Some("wip/autonoma em conflito com a base".into());
        assert_eq!(decide(&s).refusals[0].reason, "conflict");

        let mut s = snap(SAB);
        s.projects[0].wip = WipUsage {
            sessions: 1,
            prs: 0,
            limits: WipLimits {
                max_sessions: Some(1),
                max_prs: None,
            },
        };
        let o = decide(&s);
        assert_eq!(o.refusals[0].reason, "wip_limit");
        assert!(starts(&o).is_empty());

        let mut s = snap(SAB);
        s.accounts
            .insert("conta-a".into(), account(96.0, 10.0, SAB));
        let o = decide(&s);
        assert_eq!(o.refusals[0].reason, "budget:five_hour_over_cap");
    }

    #[test]
    fn sessao_viva_na_issue_nao_reinicia() {
        let mut s = snap(SAB);
        s.settings.max_sessions_per_env = 3;
        s.sessions
            .push(session("s1", "env-mac", 123, SessionState::Running));
        let o = decide(&s);
        assert!(starts(&o).is_empty());
        assert_eq!(o.projects[0].queue[0].skip.as_deref(), Some("live_session"));
    }

    #[test]
    fn limite_real_estaciona_e_retoma_depois_do_reset() {
        // Conta em 100% da 5h: a sessão ociosa estaciona até o reset.
        let mut s = snap(SAB);
        s.accounts
            .insert("conta-a".into(), account(100.0, 40.0, SAB));
        s.sessions
            .push(session("s1", "env-auto", 123, SessionState::Idle));
        let o = decide(&s);
        let parked = match &o.actions[..] {
            [Action::Park(p)] => p.clone(),
            a => panic!("{a:?}"),
        };
        assert_eq!(parked.window, "five_hour");
        assert_eq!(parked.resume_at, SAB + 2 * H + RESUME_MARGIN_MS);
        // Sessão rodando não estaciona (ainda trabalha).
        s.sessions[0].state = SessionState::Running;
        assert!(decide(&s).actions.is_empty());

        // Antes do reset: espera, e nada inicia na conta.
        let later = SAB + H;
        let mut s = snap(later);
        s.settings.max_sessions_per_env = 5;
        s.accounts
            .insert("conta-a".into(), account(100.0, 40.0, later));
        s.sessions
            .push(session("s1", "env-auto", 123, SessionState::Idle));
        s.parked.push(parked.clone());
        let o = decide(&s);
        assert!(o.actions.is_empty(), "{:?}", o.actions);
        assert_eq!(o.projects[0].blocked.as_deref(), Some("parked"));

        // Depois do reset (a leitura velha já renovou): retoma pelo texto
        // fixo, antes de iniciar outra.
        let after = parked.resume_at + 1;
        let mut s = snap(after);
        s.settings.max_sessions_per_env = 5;
        s.sessions
            .push(session("s1", "env-auto", 123, SessionState::Idle));
        s.parked.push(parked.clone());
        let o = decide(&s);
        assert_eq!(
            o.actions,
            vec![Action::Resume {
                parked: parked.clone(),
                via: ResumeVia::Continue
            }]
        );
        assert!(starts(&o).is_empty());

        // Sessão encerrada nesse meio-tempo: `session.resume`.
        s.sessions[0].state = SessionState::Exited;
        assert!(matches!(
            decide(&s).actions[..],
            [Action::Resume {
                via: ResumeVia::Resume,
                ..
            }]
        ));
        // Ambiente offline: espera.
        s.envs.get_mut("env-auto").unwrap().online = false;
        assert!(decide(&s).actions.is_empty());

        // A semanal esgotou também: estaciona de novo até o reset dela.
        let mut s = snap(after);
        s.accounts
            .insert("conta-a".into(), account(10.0, 100.0, after));
        s.sessions
            .push(session("s1", "env-auto", 123, SessionState::Idle));
        s.parked.push(parked.clone());
        match &decide(&s).actions[..] {
            [Action::Park(p)] => {
                assert_eq!(p.window, "seven_day");
                assert_eq!(p.resume_at, after + 3 * 24 * H + RESUME_MARGIN_MS);
            }
            a => panic!("{a:?}"),
        }

        // Pausado: a retomada continua (a sessão termina a issue).
        let mut s = snap(after);
        s.settings.paused = true;
        s.sessions
            .push(session("s1", "env-auto", 123, SessionState::Idle));
        s.parked.push(parked.clone());
        assert!(matches!(decide(&s).actions[..], [Action::Resume { .. }]));

        // Sessão sumiu: sai do estacionamento.
        let mut s = snap(after);
        s.parked.push(parked.clone());
        assert!(matches!(
            decide(&s).actions[..],
            [Action::Unpark { .. }, ..]
        ));
    }

    #[test]
    fn janela_esgotada_vem_da_leitura_valida() {
        assert_eq!(exhausted(&plan(99.0, 50.0, SAB)), None);
        assert_eq!(
            exhausted(&plan(100.0, 100.0, SAB)).unwrap().window,
            "seven_day"
        );
        let renovada = PlanView {
            five_hour: WindowView::Renewed {
                resets_at: SAB - 1,
                read_at: SAB - H,
            },
            seven_day: WindowView::None,
            read_at: None,
        };
        assert_eq!(exhausted(&renovada), None);
        assert_eq!(
            budget_reason(&[BlockReason::NoBand, BlockReason::AfterCutoff]),
            "budget:after_cutoff+no_band"
        );
    }

    fn account_view(v: PlanView, now: i64) -> AccountSnap {
        AccountSnap {
            decision: Some(evaluate(&BudgetPolicy::default(), &v, now)),
            exhausted: exhausted(&v),
        }
    }

    fn probes(o: &Outcome) -> Vec<(i64, bool)> {
        o.actions
            .iter()
            .filter_map(|a| match a {
                Action::Start(i) => Some((i.number, i.probe)),
                _ => None,
            })
            .collect()
    }

    const SEM_LEITURA: PlanView = PlanView {
        five_hour: WindowView::None,
        seven_day: WindowView::None,
        read_at: None,
    };

    #[test]
    fn sem_leitura_e_sem_sessao_viva_inicia_a_sonda() {
        let mut s = snap(SAB);
        s.accounts
            .insert("conta-a".into(), account_view(SEM_LEITURA, SAB));
        let o = decide(&s);
        assert_eq!(probes(&o), vec![(123, true)]);
        assert!(o.refusals.is_empty(), "{:?}", o.refusals);
        assert!(o.projects[0].probe);
        assert_eq!(o.projects[0].blocked, None);
        // Leitura normal: início comum, sem sonda.
        let o = decide(&snap(SAB));
        assert_eq!(probes(&o), vec![(123, false)]);
        assert!(!o.projects[0].probe);
    }

    #[test]
    fn sem_leitura_com_sessao_viva_na_conta_espera_a_leitura() {
        let mut s = snap(SAB);
        s.accounts
            .insert("conta-a".into(), account_view(SEM_LEITURA, SAB));
        // Outro ambiente autônomo da mesma conta, com vaga própria: a sonda
        // é uma por conta, não por ambiente.
        s.envs.insert(
            "env-auto-2".into(),
            EnvSnap {
                name: "autonomo-2".into(),
                online: true,
                autonomous: true,
                account: Some("conta-a".into()),
            },
        );
        s.sessions
            .push(session("s1", "env-auto-2", 99, SessionState::Starting));
        let o = decide(&s);
        assert!(starts(&o).is_empty());
        assert_eq!(o.projects[0].blocked.as_deref(), Some("budget:no_reading"));
        assert!(!o.projects[0].probe);
        assert_eq!(o.refusals[0].scope, "account:conta-a");
        assert!(o.refusals[0].message.contains("sonda espera"));
        // A sessão encerrada não conta: a sonda volta a poder.
        s.sessions[0].state = SessionState::Archived;
        assert_eq!(probes(&decide(&s)), vec![(123, true)]);
    }

    #[test]
    fn sonda_respeita_o_resto_da_politica() {
        // Depois do corte de início, sem leitura: nada inicia.
        let mut s = snap(TER_CORTE);
        s.accounts
            .insert("conta-a".into(), account_view(SEM_LEITURA, TER_CORTE));
        let o = decide(&s);
        assert!(starts(&o).is_empty());
        assert_eq!(
            o.projects[0].blocked.as_deref(),
            Some("budget:after_cutoff+no_reading")
        );
        // Pausado: nada inicia.
        let mut s = snap(SAB);
        s.accounts
            .insert("conta-a".into(), account_view(SEM_LEITURA, SAB));
        s.settings.paused = true;
        assert!(starts(&decide(&s)).is_empty());
    }

    #[test]
    fn janela_renovada_sem_leitura_nova_conta_como_zero() {
        // A 5h estava em 99% e renovou; a semanal segue válida e baixa.
        let mut s = snap(SAB);
        let mut v = plan(10.0, 10.0, SAB);
        v.five_hour = WindowView::Renewed {
            resets_at: SAB - H,
            read_at: SAB - 4 * H,
        };
        s.accounts.insert("conta-a".into(), account_view(v, SAB));
        let o = decide(&s);
        assert_eq!(probes(&o), vec![(123, false)]);
        assert_eq!(
            s.accounts["conta-a"]
                .decision
                .as_ref()
                .unwrap()
                .five_hour_used,
            Some(0.0)
        );
    }

    #[test]
    fn erro_na_branch_de_integracao_bloqueia_o_projeto() {
        let mut s = snap(SAB);
        s.projects[0].autonomy_error = Some("push recusado: 403".into());
        let o = decide(&s);
        assert!(starts(&o).is_empty());
        assert_eq!(o.projects[0].blocked.as_deref(), Some("autonomy_error"));
        assert_eq!(o.refusals[0].reason, "autonomy_error");
        assert_eq!(o.refusals[0].scope, "project:acme/acme-web");
        assert!(o.refusals[0].message.contains("403"));
        // Sem leitura, o erro também barra a sonda.
        s.accounts
            .insert("conta-a".into(), account_view(SEM_LEITURA, SAB));
        let o = decide(&s);
        assert!(starts(&o).is_empty());
        assert!(!o.projects[0].probe);
        assert_eq!(o.projects[0].blocked.as_deref(), Some("autonomy_error"));
    }
}
