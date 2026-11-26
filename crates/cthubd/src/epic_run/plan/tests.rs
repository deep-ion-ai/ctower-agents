use super::*;
use crate::autopilot::plan::{FEEDBACK_CI, MERGE_GREEN};
use ct_core::{EpicPlanState, PrState};
use ct_hub_store::{EpicPlanEdge, EpicPlanNode};
use ct_ledger::{CheckSource, PrCheck, PrChecks, PrFeedback, PrRow};

const T: i64 = 1_000_000_000;
const WIP: &str = "wip/10-epico";

fn run(status: EpicRunStatus) -> EpicRun {
    EpicRun {
        id: 1,
        project_id: 1,
        owner: "acme".into(),
        repo: "acme-web".into(),
        epic: 10,
        env_id: "env-a".into(),
        status,
        plan_version: Some(1),
        max_parallel: 2,
        code_review: false,
        started_by: "fulano".into(),
        plan_session: None,
        plan_model: None,
        plan_effort: None,
        paused_reason: None,
        resume_at: None,
        created_at: 0,
        updated_at: 0,
        ended_at: None,
    }
}

fn pnode(n: i64, areas: &[&str]) -> EpicPlanNode {
    EpicPlanNode {
        number: n,
        position: n,
        model: Some("sonnet".into()),
        effort: Some("medium".into()),
        areas: areas.iter().map(|a| a.to_string()).collect(),
        reason: String::new(),
        fingerprint: Some(format!("f{n}")),
    }
}

fn edge(n: i64, b: i64) -> EpicPlanEdge {
    EpicPlanEdge {
        number: n,
        blocker: b,
        reason: String::new(),
        source: "analysis".into(),
    }
}

fn node(n: i64, state: EpicNodeState) -> EpicNode {
    EpicNode {
        run_id: 1,
        number: n,
        state,
        prev_state: None,
        session_id: None,
        worktree: None,
        pr: None,
        attempts: 0,
        assumed_since: None,
        reason: None,
        rebase_head: None,
        rebase_at: None,
        updated_at: T - 60_000,
    }
}

/// Execução em `status` com o plano `nodes` (todos pendentes, com título e
/// a impressão digital do plano) e as arestas.
fn snap(status: EpicRunStatus, plan: Vec<EpicPlanNode>, edges: Vec<EpicPlanEdge>) -> Snapshot {
    let items = plan
        .iter()
        .map(|n| {
            (
                n.number,
                ItemSnap {
                    title: Some(format!("Sub-issue {}", n.number)),
                    fingerprint: n.fingerprint.clone(),
                    ..Default::default()
                },
            )
        })
        .collect();
    let nodes = plan
        .iter()
        .map(|n| node(n.number, EpicNodeState::Pendente))
        .collect();
    Snapshot {
        now: T,
        run: run(status),
        plan: Some(EpicPlan {
            run_id: 1,
            version: 1,
            state: EpicPlanState::Aprovado,
            summary: String::new(),
            nodes: plan,
            edges,
            proposals: vec![],
            session_id: None,
            created_at: 0,
            approved_at: Some(0),
            approved_by: None,
        }),
        nodes,
        wip: WIP.into(),
        items,
        external: BTreeMap::new(),
        sessions: BTreeMap::new(),
        env_online: true,
        exhausted: None,
        pending: HashSet::new(),
        max_ci_attempts: 2,
        idle_timeout_min: 30,
        handback_at: BTreeMap::new(),
    }
}

fn set(s: &mut Snapshot, n: i64, f: impl FnOnce(&mut EpicNode)) {
    f(s.nodes.iter_mut().find(|x| x.number == n).unwrap());
}

/// Nó `n` em `state` com a sessão `s-n` no estado dado (ociosa há 1 min).
fn with_session(s: &mut Snapshot, n: i64, state: EpicNodeState, sess: SessionState) {
    set(s, n, |x| {
        x.state = state;
        x.session_id = Some(format!("s-{n}"));
        x.worktree = Some(format!("wt/{n}"));
    });
    s.sessions.insert(
        format!("s-{n}"),
        SessionSnap {
            state: sess,
            idle_since: (sess == SessionState::Idle).then_some(T - 60_000),
            human_input_at: None,
        },
    );
}

fn pr(n: i64, head: &str) -> PrRow {
    PrRow {
        number: 100 + n,
        work_item_id: n,
        head: format!("feat/{n}"),
        base: WIP.into(),
        state: PrState::Open,
        url: String::new(),
        updated_at: 50,
        head_sha: Some(head.into()),
    }
}

fn checks(n: i64, head: &str, conclusion: &str) -> PrChecks {
    let c = vec![PrCheck {
        name: "rust".into(),
        source: CheckSource::CheckRun,
        conclusion: Some(conclusion.into()),
        url: None,
        job_id: Some(1),
    }];
    PrChecks {
        pr_number: 100 + n,
        head: format!("feat/{n}"),
        base: WIP.into(),
        head_sha: head.into(),
        state: ct_ledger::checks_state(&c),
        checks: c,
        synced_at: 1,
        runs_etag: None,
        status_etag: None,
    }
}

/// PR aberto do nó `n` na `wip` no head `head`, checks com `conclusion`.
fn open_pr(s: &mut Snapshot, n: i64, head: &str, conclusion: &str, close: CloseState) {
    s.items.get_mut(&n).unwrap().open = Some(Closing {
        work_item_id: n,
        integration: WIP.into(),
        pr: pr(n, head),
        checks: Some(checks(n, head, conclusion)),
        feedback: Some(PrFeedback {
            pr: pr(n, head),
            read_at_updated: Some(50),
            threads: vec![],
            changes_requested: vec![],
        }),
        state: close,
    });
}

fn starts(o: &Outcome) -> Vec<i64> {
    o.actions
        .iter()
        .filter_map(|a| match a {
            Action::Start { number, .. } => Some(*number),
            _ => None,
        })
        .collect()
}

fn node_moves(o: &Outcome) -> Vec<(i64, EpicNodeState)> {
    o.actions
        .iter()
        .filter_map(|a| match a {
            Action::Node { number, to, .. } => Some((*number, *to)),
            _ => None,
        })
        .collect()
}

fn asks(o: &Outcome) -> Vec<(Option<i64>, EpicDecisionOrigin, bool)> {
    o.actions
        .iter()
        .filter_map(|a| match a {
            Action::Ask {
                number,
                origin,
                block,
                ..
            } => Some((*number, *origin, *block)),
            _ => None,
        })
        .collect()
}

fn status_to(o: &Outcome) -> Vec<(EpicRunStatus, Option<i64>)> {
    o.actions
        .iter()
        .filter_map(|a| match a {
            Action::Status { to, resume_at, .. } => Some((*to, *resume_at)),
            _ => None,
        })
        .collect()
}

// --- Pronta ---------------------------------------------------------------

#[test]
fn pronta_quando_os_bloqueantes_estao_mesclados() {
    // (bloqueante no épico mesclado?, externo pronto?) -> 12 fica pronta?
    let table = [
        (false, Some(true), false),
        (true, None, false),
        (true, Some(false), false),
        (true, Some(true), true),
    ];
    for (blocker_merged, external, ready) in table {
        let mut s = snap(
            EpicRunStatus::Executando,
            vec![pnode(11, &[]), pnode(12, &[])],
            vec![edge(12, 11), edge(12, 99)],
        );
        if blocker_merged {
            set(&mut s, 11, |x| x.state = EpicNodeState::Mesclada);
        }
        if let Some(e) = external {
            s.external.insert(99, e);
        }
        let o = decide(&s);
        assert_eq!(
            node_moves(&o).contains(&(12, EpicNodeState::Pronta)),
            ready,
            "{blocker_merged} {external:?}"
        );
    }
}

#[test]
fn pronta_que_perde_o_bloqueante_volta_a_pendente() {
    let mut s = snap(
        EpicRunStatus::Executando,
        vec![pnode(11, &[]), pnode(12, &[])],
        vec![edge(12, 99)],
    );
    set(&mut s, 12, |x| x.state = EpicNodeState::Pronta);
    s.external.insert(99, false);
    let o = decide(&s);
    assert!(node_moves(&o).contains(&(12, EpicNodeState::Pendente)));
    assert!(!starts(&o).contains(&12));
}

// --- Início ---------------------------------------------------------------

#[test]
fn inicia_na_ordem_do_plano_ate_o_paralelismo() {
    let mut s = snap(
        EpicRunStatus::Executando,
        vec![pnode(11, &[]), pnode(12, &[]), pnode(13, &[])],
        vec![],
    );
    let o = decide(&s);
    assert_eq!(starts(&o), vec![11, 12]);
    assert_eq!(o.blocked, Some("full"));
    let Some(Action::Start { model, effort, .. }) =
        o.actions.iter().find(|a| matches!(a, Action::Start { .. }))
    else {
        panic!()
    };
    assert_eq!(
        (model.as_str(), effort.as_deref()),
        ("sonnet", Some("medium"))
    );
    // Um nó vivo ocupa uma vaga.
    with_session(&mut s, 11, EpicNodeState::EmExecucao, SessionState::Running);
    assert_eq!(starts(&decide(&s)), vec![12]);
}

#[test]
fn areas_sobrepostas_sem_aresta_nao_rodam_juntas() {
    // (áreas da 11, áreas da 12) -> inicia as duas?
    let table: [(&[&str], &[&str], bool); 5] = [
        (&["crates/a/"], &["crates/b/"], true),
        (&["crates/a/"], &["crates/a/src/x.rs"], false),
        (&["crates/**"], &["crates/b/"], false),
        (&[], &["crates/a/"], true),
        (&["ui/src/*.ts"], &["ui/src/main.ts"], false),
    ];
    for (a, b, both) in table {
        let s = snap(
            EpicRunStatus::Executando,
            vec![pnode(11, a), pnode(12, b)],
            vec![],
        );
        let st = starts(&decide(&s));
        assert_eq!(st.len() == 2, both, "{a:?} {b:?}");
        assert_eq!(st[0], 11);
    }
    // Com a 11 viva, a 12 sobreposta espera; a 13 livre inicia.
    let mut s = snap(
        EpicRunStatus::Executando,
        vec![
            pnode(11, &["crates/a/"]),
            pnode(12, &["crates/a/"]),
            pnode(13, &["docs/"]),
        ],
        vec![],
    );
    s.run.max_parallel = 3;
    with_session(&mut s, 11, EpicNodeState::EmPr, SessionState::Running);
    assert_eq!(starts(&decide(&s)), vec![13]);
}

#[test]
fn nada_inicia_pausado_esgotado_offline_ou_com_ciclo() {
    let base = || {
        snap(
            EpicRunStatus::Executando,
            vec![pnode(11, &[]), pnode(12, &[])],
            vec![],
        )
    };
    let mut s = base();
    s.run.status = EpicRunStatus::Pausado;
    assert_eq!(
        (starts(&decide(&s)), decide(&s).blocked),
        (vec![], Some("paused"))
    );
    let mut s = base();
    s.env_online = false;
    assert_eq!(decide(&s).blocked, Some("env_offline"));
    assert!(starts(&decide(&s)).is_empty());
    let mut s = base();
    s.exhausted = Some(Exhausted {
        window: "five_hour".into(),
        resets_at: T + 3_600_000,
    });
    assert!(starts(&decide(&s)).is_empty());
    let mut s = base();
    s.plan.as_mut().unwrap().edges = vec![edge(11, 12), edge(12, 11)];
    let o = decide(&s);
    assert!(starts(&o).is_empty());
    assert_eq!(o.blocked, Some("cycle"));
    assert_eq!(o.cycle, [11, 12].into());
    let mut s = base();
    s.run.status = EpicRunStatus::AguardandoAprovacao;
    assert!(starts(&decide(&s)).is_empty());
}

#[test]
fn no_com_pedido_pendente_nao_inicia() {
    let mut s = snap(
        EpicRunStatus::Executando,
        vec![pnode(11, &[]), pnode(12, &[])],
        vec![],
    );
    s.pending
        .insert((Some(11), EpicDecisionOrigin::IssueAlterada));
    assert_eq!(starts(&decide(&s)), vec![12]);
}

// --- Fechamento -----------------------------------------------------------

#[test]
fn pr_aberto_passa_o_no_para_em_pr() {
    let mut s = snap(EpicRunStatus::Executando, vec![pnode(11, &[])], vec![]);
    with_session(&mut s, 11, EpicNodeState::EmExecucao, SessionState::Running);
    open_pr(&mut s, 11, "h1", "success", CloseState::default());
    let o = decide(&s);
    assert_eq!(node_moves(&o), vec![(11, EpicNodeState::EmPr)]);
    assert!(o
        .actions
        .iter()
        .any(|a| matches!(a, Action::Node { pr: Some(111), .. })));
}

#[test]
fn ci_falho_pede_correcao_e_esgotado_pede_decisao() {
    let mut s = snap(EpicRunStatus::Executando, vec![pnode(11, &[])], vec![]);
    with_session(&mut s, 11, EpicNodeState::EmPr, SessionState::Idle);
    open_pr(&mut s, 11, "h1", "failure", CloseState::default());
    let o = decide(&s);
    let Some(Action::Send {
        kind, close, text, ..
    }) = o.actions.iter().find(|a| matches!(a, Action::Send { .. }))
    else {
        panic!("{o:?}")
    };
    assert_eq!(*kind, SendKind::Feedback(FEEDBACK_CI));
    assert_eq!(close.attempts, 1);
    assert!(text.starts_with(EPIC_FEEDBACK_REQUEST), "{text}");
    // Esgotou as tentativas: pedido de decisão, nó bloqueado, sessão viva.
    open_pr(
        &mut s,
        11,
        "h2",
        "failure",
        CloseState {
            attempts: 2,
            feedback_head: Some("h1".into()),
            ..Default::default()
        },
    );
    let o = decide(&s);
    assert_eq!(
        asks(&o),
        vec![(Some(11), EpicDecisionOrigin::CiEsgotado, true)]
    );
    assert!(!o.actions.iter().any(|a| matches!(a, Action::End { .. })));
}

#[test]
fn verde_pede_merge_com_rebase_na_wip() {
    let mut s = snap(EpicRunStatus::Executando, vec![pnode(11, &[])], vec![]);
    with_session(&mut s, 11, EpicNodeState::EmPr, SessionState::Idle);
    open_pr(&mut s, 11, "h1", "success", CloseState::default());
    let o = decide(&s);
    let Some(Action::Send { kind, text, .. }) =
        o.actions.iter().find(|a| matches!(a, Action::Send { .. }))
    else {
        panic!("{o:?}")
    };
    assert_eq!(*kind, SendKind::Merge);
    assert!(
        text.contains("gh pr merge 111 --rebase") && text.contains(WIP),
        "{text}"
    );
    let _ = MERGE_GREEN;
}

#[test]
fn ociosa_sem_pr_alem_do_prazo_pede_decisao() {
    let mut s = snap(EpicRunStatus::Executando, vec![pnode(11, &[])], vec![]);
    with_session(&mut s, 11, EpicNodeState::EmExecucao, SessionState::Idle);
    assert!(asks(&decide(&s)).is_empty());
    s.sessions.get_mut("s-11").unwrap().idle_since = Some(T - 31 * 60_000);
    assert_eq!(
        asks(&decide(&s)),
        vec![(Some(11), EpicDecisionOrigin::SemProgresso, true)]
    );
    s.idle_timeout_min = 0;
    assert!(asks(&decide(&s)).is_empty());
}

#[test]
fn sessao_sumida_pede_decisao_depois_da_folga() {
    let mut s = snap(EpicRunStatus::Executando, vec![pnode(11, &[])], vec![]);
    set(&mut s, 11, |x| {
        x.state = EpicNodeState::EmExecucao;
        x.updated_at = T - 60_000;
    });
    // Recém-iniciada, sessão ainda desconhecida: espera.
    assert!(asks(&decide(&s)).is_empty());
    set(&mut s, 11, |x| x.updated_at = T - START_GRACE_MS);
    assert_eq!(
        asks(&decide(&s)),
        vec![(Some(11), EpicDecisionOrigin::SemProgresso, true)]
    );
    // Sessão conhecida e encerrada: pede na hora.
    let mut s = snap(EpicRunStatus::Executando, vec![pnode(11, &[])], vec![]);
    with_session(&mut s, 11, EpicNodeState::EmPr, SessionState::Exited);
    assert_eq!(asks(&decide(&s)).len(), 1);
    s.pending
        .insert((Some(11), EpicDecisionOrigin::SemProgresso));
    assert!(asks(&decide(&s)).is_empty());
}

// --- Merge, worktree e rebase -------------------------------------------

#[test]
fn merge_encerra_a_sessao_e_remove_o_worktree_uma_vez() {
    let mut s = snap(
        EpicRunStatus::Executando,
        vec![pnode(11, &[]), pnode(12, &[])],
        vec![],
    );
    with_session(&mut s, 11, EpicNodeState::EmPr, SessionState::Idle);
    s.items.get_mut(&11).unwrap().merged = Some(111);
    let o = decide(&s);
    assert!(node_moves(&o).contains(&(11, EpicNodeState::Mesclada)));
    assert!(o
        .actions
        .iter()
        .any(|a| matches!(a, Action::End { number: 11, .. })));
    // Ainda com a sessão viva: não remove.
    assert!(!o
        .actions
        .iter()
        .any(|a| matches!(a, Action::RemoveWorktree { .. })));
    // Ciclo seguinte: mesclada, sessão encerrada, worktree no nó.
    set(&mut s, 11, |x| x.state = EpicNodeState::Mesclada);
    s.sessions.get_mut("s-11").unwrap().state = SessionState::Exited;
    let removes = |o: &Outcome| {
        o.actions
            .iter()
            .filter(|a| matches!(a, Action::RemoveWorktree { number: 11, .. }))
            .count()
    };
    let o = decide(&s);
    assert_eq!(removes(&o), 1);
    // O runner limpou o worktree (removido ou recusado): não pede de novo,
    // e a execução segue (a 12 inicia).
    set(&mut s, 11, |x| {
        x.worktree = None;
        x.reason = Some("mudança não commitada".into());
    });
    let o = decide(&s);
    assert_eq!(removes(&o), 0);
    assert_eq!(starts(&o), vec![12]);
}

#[test]
fn merge_pede_rebase_aos_nos_em_pr() {
    let mut s = snap(
        EpicRunStatus::Executando,
        vec![
            pnode(11, &[]),
            pnode(12, &[]),
            pnode(13, &[]),
            pnode(14, &[]),
        ],
        vec![],
    );
    s.run.max_parallel = 4;
    with_session(&mut s, 11, EpicNodeState::EmPr, SessionState::Idle);
    s.items.get_mut(&11).unwrap().merged = Some(111);
    with_session(&mut s, 12, EpicNodeState::EmPr, SessionState::Idle);
    open_pr(&mut s, 12, "h12", "success", CloseState::default());
    with_session(&mut s, 13, EpicNodeState::EmPr, SessionState::Running);
    open_pr(&mut s, 13, "h13", "success", CloseState::default());
    with_session(&mut s, 14, EpicNodeState::Assumida, SessionState::Idle);
    open_pr(&mut s, 14, "h14", "success", CloseState::default());
    let o = decide(&s);
    let rebases: Vec<(i64, bool, String)> = o
        .actions
        .iter()
        .filter_map(|a| match a {
            Action::Rebase {
                number,
                session_id,
                head,
                ..
            } => Some((*number, session_id.is_some(), head.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        rebases,
        vec![(12, true, "h12".into()), (13, false, "h13".into())]
    );
    // O rebase pedido segura o pedido de merge do mesmo ciclo.
    assert!(!o
        .actions
        .iter()
        .any(|a| matches!(a, Action::Send { number: 12, .. })));
}

#[test]
fn rebase_pendente_enviado_concluido_ou_em_conflito() {
    let mut s = snap(EpicRunStatus::Executando, vec![pnode(12, &[])], vec![]);
    with_session(&mut s, 12, EpicNodeState::EmPr, SessionState::Idle);
    open_pr(&mut s, 12, "h12", "success", CloseState::default());
    // Pendente (sessão estava ocupada), agora ociosa: envia.
    set(&mut s, 12, |x| x.rebase_head = Some("h12".into()));
    let o = decide(&s);
    assert!(o.actions.iter().any(|a| matches!(
        a,
        Action::Rebase {
            number: 12,
            session_id: Some(_),
            ..
        }
    )));
    // Enviado e o PR não mudou: espera o prazo, depois pede decisão.
    set(&mut s, 12, |x| x.rebase_at = Some(T - 60_000));
    assert!(asks(&decide(&s)).is_empty());
    set(&mut s, 12, |x| x.rebase_at = Some(T - 31 * 60_000));
    s.sessions.get_mut("s-12").unwrap().idle_since = Some(T - 31 * 60_000);
    assert_eq!(
        asks(&decide(&s)),
        vec![(Some(12), EpicDecisionOrigin::ConflitoRebase, true)]
    );
    // Head novo: rebase feito.
    open_pr(&mut s, 12, "h12b", "success", CloseState::default());
    let o = decide(&s);
    assert!(o
        .actions
        .iter()
        .any(|a| matches!(a, Action::RebaseDone { number: 12 })));
}

// --- Assumida -------------------------------------------------------------

#[test]
fn assumida_nao_recebe_nada_nem_e_encerrada() {
    let mut s = snap(EpicRunStatus::Executando, vec![pnode(11, &[])], vec![]);
    with_session(&mut s, 11, EpicNodeState::Assumida, SessionState::Idle);
    s.sessions.get_mut("s-11").unwrap().idle_since = Some(T - 300 * 60_000);
    open_pr(&mut s, 11, "h1", "failure", CloseState::default());
    let o = decide(&s);
    assert!(o.actions.is_empty(), "{o:?}");
    // Assumida ocupa vaga e áreas.
    assert_eq!(o.blocked, None);
    // O merge ainda marca o nó, sem encerrar a sessão.
    s.items.get_mut(&11).unwrap().merged = Some(111);
    let o = decide(&s);
    assert_eq!(node_moves(&o), vec![(11, EpicNodeState::Mesclada)]);
    assert!(!o.actions.iter().any(|a| matches!(a, Action::End { .. })));
}

// --- Orçamento, pausa e estacionamento ----------------------------------

#[test]
fn janela_esgotada_estaciona_com_margem_de_5_min() {
    let mut s = snap(
        EpicRunStatus::Executando,
        vec![pnode(11, &[]), pnode(12, &[])],
        vec![],
    );
    with_session(&mut s, 11, EpicNodeState::EmPr, SessionState::Idle);
    with_session(&mut s, 12, EpicNodeState::EmExecucao, SessionState::Running);
    let resets = T + 3_600_000;
    s.exhausted = Some(Exhausted {
        window: "five_hour".into(),
        resets_at: resets,
    });
    let o = decide(&s);
    assert_eq!(
        status_to(&o),
        vec![(EpicRunStatus::Estacionado, Some(resets + 5 * 60_000))]
    );
    // A ociosa estaciona; a que roda termina o turno.
    assert_eq!(node_moves(&o), vec![(11, EpicNodeState::Estacionada)]);
    assert!(!o.actions.iter().any(|a| matches!(a, Action::Send { .. })));
}

#[test]
fn estacionada_retoma_so_depois_de_resets_mais_5_min() {
    let resets = T - 3_600_000;
    let resume = resets + RESUME_MARGIN_MS;
    // (agora, janela ainda esgotada?) -> status, reagendou?
    let table = [
        (resume - 1, false, vec![], false),
        (
            resume,
            false,
            vec![(EpicRunStatus::Executando, None)],
            false,
        ),
        (resume + 10, true, vec![], true),
    ];
    for (now, still, want, reschedule) in table {
        let mut s = snap(
            EpicRunStatus::Estacionado,
            vec![pnode(11, &[]), pnode(12, &[])],
            vec![],
        );
        s.run.resume_at = Some(resume);
        s.now = now;
        with_session(&mut s, 11, EpicNodeState::Estacionada, SessionState::Idle);
        if still {
            s.exhausted = Some(Exhausted {
                window: "seven_day".into(),
                resets_at: now + 86_400_000,
            });
        }
        let o = decide(&s);
        assert_eq!(status_to(&o), want, "{now}");
        assert_eq!(
            o.actions
                .iter()
                .any(|a| matches!(a, Action::Reschedule { .. })),
            reschedule
        );
        // Retoma antes de iniciar nó novo: nada inicia no ciclo.
        assert!(starts(&o).is_empty());
        let unparked = o.actions.iter().any(|a| {
            matches!(
                a,
                Action::Unpark {
                    number: 11,
                    via: ResumeVia::Continue,
                    ..
                }
            )
        });
        assert_eq!(unparked, !want.is_empty());
    }
}

#[test]
fn pausa_manual_nao_retoma_com_o_reset() {
    let mut s = snap(
        EpicRunStatus::Pausado,
        vec![pnode(11, &[]), pnode(12, &[])],
        vec![],
    );
    s.run.resume_at = None;
    with_session(&mut s, 11, EpicNodeState::EmPr, SessionState::Idle);
    with_session(&mut s, 12, EpicNodeState::Estacionada, SessionState::Idle);
    let o = decide(&s);
    assert!(status_to(&o).is_empty());
    assert_eq!(node_moves(&o), vec![(11, EpicNodeState::Estacionada)]);
    assert!(!o.actions.iter().any(|a| matches!(a, Action::Unpark { .. })));
    // Retomada pelo desenvolvedor (status executando): os estacionados
    // voltam; a sessão encerrada volta com resume.
    s.run.status = EpicRunStatus::Executando;
    s.sessions.get_mut("s-12").unwrap().state = SessionState::Exited;
    let o = decide(&s);
    assert!(o.actions.iter().any(|a| matches!(
        a,
        Action::Unpark {
            number: 12,
            via: ResumeVia::Resume,
            ..
        }
    )));
}

// --- Sub-issue alterada e integração ------------------------------------

#[test]
fn sub_issue_alterada_pede_decisao_uma_vez() {
    let mut s = snap(EpicRunStatus::Executando, vec![pnode(11, &[])], vec![]);
    with_session(&mut s, 11, EpicNodeState::EmExecucao, SessionState::Running);
    s.items.get_mut(&11).unwrap().fingerprint = Some("outra".into());
    assert_eq!(
        asks(&decide(&s)),
        vec![(Some(11), EpicDecisionOrigin::IssueAlterada, false)]
    );
    s.pending
        .insert((Some(11), EpicDecisionOrigin::IssueAlterada));
    assert!(asks(&decide(&s)).is_empty());
}

#[test]
fn todas_mescladas_vai_para_integrando() {
    let mut s = snap(
        EpicRunStatus::Executando,
        vec![pnode(11, &[]), pnode(12, &[])],
        vec![],
    );
    set(&mut s, 11, |x| x.state = EpicNodeState::Mesclada);
    with_session(&mut s, 12, EpicNodeState::EmPr, SessionState::Idle);
    assert!(status_to(&decide(&s)).is_empty());
    s.items.get_mut(&12).unwrap().merged = Some(112);
    assert_eq!(
        status_to(&decide(&s)),
        vec![(EpicRunStatus::Integrando, None)]
    );
}

#[test]
fn sem_plano_aprovado_nada_acontece() {
    let mut s = snap(EpicRunStatus::Planejando, vec![pnode(11, &[])], vec![]);
    s.plan = None;
    let o = decide(&s);
    assert!(o.actions.is_empty());
    let mut s = snap(EpicRunStatus::Cancelado, vec![pnode(11, &[])], vec![]);
    s.items.get_mut(&11).unwrap().merged = Some(111);
    assert!(decide(&s).actions.is_empty());
}

// --- Assumida pela entrada humana (#301) --------------------------------

#[test]
fn entrada_humana_assume_e_o_handback_devolve() {
    let mut s = snap(EpicRunStatus::Executando, vec![pnode(11, &[])], vec![]);
    with_session(&mut s, 11, EpicNodeState::EmPr, SessionState::Idle);
    s.sessions.get_mut("s-11").unwrap().idle_since = Some(T - 300 * 60_000);
    open_pr(&mut s, 11, "h1", "failure", CloseState::default());
    s.sessions.get_mut("s-11").unwrap().human_input_at = Some(T - 1_000);
    let o = decide(&s);
    assert_eq!(node_moves(&o), vec![(11, EpicNodeState::Assumida)]);
    // Nada vai à sessão no mesmo ciclo, nem pedido de decisão.
    assert!(!o
        .actions
        .iter()
        .any(|a| matches!(a, Action::Send { .. } | Action::Ask { .. })));
    // Devolvido depois da entrada: o motor volta a dirigir (pede correção).
    s.handback_at.insert(11, T - 500);
    let o = decide(&s);
    assert!(node_moves(&o).is_empty());
    assert!(o.actions.iter().any(|a| matches!(a, Action::Send { .. })));
    // Assumida não volta a "assumir".
    set(&mut s, 11, |x| x.state = EpicNodeState::Assumida);
    s.handback_at.clear();
    assert!(node_moves(&decide(&s)).is_empty());
}
