//! A decisão do motor de execução de épico (#297, épico #295), pura: recebe
//! a fotografia de uma execução ([`Snapshot`]) e devolve o que fazer
//! ([`Outcome`]). Sem I/O nem relógio: os testes montam a fotografia à mão.
//! Segue o formato do agendador do piloto automático
//! ([`crate::autopilot::plan`]) e reusa o fechamento do PR dele
//! ([`close_decision`]).
//!
//! # Regras
//!
//! - **Plano:** só a versão aprovada em vigor (congelada). Sem ela
//!   (`planejando`), nada. Ciclo nas arestas do plano: nenhum início, e os
//!   nós do ciclo saem em [`Outcome::cycle`].
//! - **Pronta:** todos os bloqueantes do nó estão `mesclada` na `wip`; um
//!   bloqueante fora do épico conta quando está mesclado na base ou
//!   concluído ([`Snapshot::external`]). Pronta que deixa de estar volta a
//!   `pendente`.
//! - **Início:** só com a execução em `executando`, o ambiente online, a
//!   conta sem janela esgotada, vaga em `max_parallel` (nós vivos:
//!   [`EpicNodeState::is_live`]) e nenhuma área sobreposta à de outro nó
//!   vivo ([`areas_overlap`]; nó sem áreas não conflita). Na ordem do plano,
//!   com o modelo e o esforço do nó. Nó com pedido de decisão pendente não
//!   inicia.
//! - **Fechamento:** o do #184 com a base `wip/<épico>`: CI falho pede
//!   correção e conta tentativa; thread de colaborador pede correção; verde
//!   pede merge com rebase. Tentativas esgotadas ou PR parado viram pedido
//!   de decisão (`ci_esgotado`, `sem_progresso`) e o nó fica `bloqueada`; a
//!   sessão continua viva para quem quiser assumir.
//! - **Merge:** PR da sub-issue mesclado na `wip` marca o nó `mesclada` e
//!   encerra a sessão; com a sessão já encerrada, pede a remoção do
//!   worktree ([`Action::RemoveWorktree`], uma vez: o runner limpa o
//!   `worktree` do nó com qualquer resultado).
//! - **Rebase:** depois de cada merge na `wip`, cada nó `em_pr` ganha um
//!   pedido de rebase, enviado com a sessão ociosa. Head novo no PR limpa o
//!   pedido; sem head novo depois do prazo de ociosidade, pedido de decisão
//!   `conflito_rebase`.
//! - **Assumida:** nada vai à sessão, ela não é encerrada nem conta sem
//!   progresso. O merge ainda marca o nó.
//! - **Sem progresso:** sessão do nó sumida (ou que não aparece
//!   [`START_GRACE_MS`] depois do início) ou ociosa sem PR por
//!   `idle_timeout_min`.
//! - **Sub-issue alterada** depois do plano (impressão digital diferente
//!   da congelada): pedido `issue_alterada`, sem parar o nó.
//! - **Orçamento:** com a janela de 5h ou a semanal em 100% antes do reset,
//!   a execução que roda estaciona com `resume_at = resets_at +`
//!   [`RESUME_MARGIN_MS`]. Pausada ou estacionada, as sessões ociosas
//!   estacionam (nó `estacionada`) e nada inicia. A estacionada retoma no
//!   `resume_at` (se a conta não esgotou de novo, senão adia), e os nós
//!   estacionados voltam antes de qualquer início, que só vem no ciclo
//!   seguinte. Pausa manual tem precedência: não retoma sozinha.
//! - **Integração:** todos os nós do plano `mesclada` → `integrando`.

use crate::autopilot::deps::cycle_members;
use crate::autopilot::plan::{
    close_decision, CloseInput, CloseStep, CloseTexts, Closing, Exhausted, ResumeVia, SendKind,
    BLOCK_ATTEMPTS,
};
use ct_core::{areas_overlap, EpicDecisionOrigin, EpicNodeState, EpicRunStatus, SessionState};
use ct_hub_store::{CloseState, EpicNode, EpicPlan, EpicRun};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// Folga depois do `resets_at` antes de retomar: o relógio do hub e o da
/// conta podem divergir.
pub const RESUME_MARGIN_MS: i64 = 5 * 60_000;
/// Nó iniciado cuja sessão o hub ainda não conhece conta como iniciando
/// por este tempo; depois, sem progresso.
pub const START_GRACE_MS: i64 = 10 * 60_000;

/// Instrução fixa do pedido de correção da execução de épico.
pub const EPIC_FEEDBACK_REQUEST: &str = "Execução de épico do Control Tower: o PR desta \
sub-issue precisa de correção (checks do CI falharam ou há review de colaborador pendente). \
Corrija, faça push na mesma branch e responda nas threads de review. Use as ferramentas \
pr_feedback e pr_check_log do Control Tower para ver os detalhes e os logs dos checks. Não \
mescle o PR agora: o hub pede o merge quando o CI passar.";

/// O pedido de merge: CI verde no PR `n`, base `branch` (a `wip`).
pub fn epic_merge_text(n: i64, branch: &str) -> String {
    format!(
        "Execução de épico do Control Tower: CI verde no PR #{n}, sem review pendente. Mescle \
         o PR #{n} na {branch} com `gh pr merge {n} --rebase` e encerre (não mescle em outra \
         branch nem abra outro PR)."
    )
}

/// Os textos do fechamento na execução de épico.
pub const EPIC_TEXTS: CloseTexts = CloseTexts {
    request: EPIC_FEEDBACK_REQUEST,
    merge: epic_merge_text,
};

/// O pedido de rebase depois do merge de `merged` na `wip`.
pub fn rebase_text(pr: i64, wip: &str, merged: &[i64]) -> String {
    let which = merged
        .iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Execução de épico do Control Tower: {which} foi mesclada na {wip}. Atualize o PR \
         #{pr} com rebase na {wip} (git fetch, git rebase origin/{wip}, resolva os conflitos e \
         faça git push --force-with-lease na mesma branch). Não mescle o PR: o hub pede o \
         merge quando o CI passar."
    )
}

/// A sessão de um nó como o hub a conhece.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSnap {
    pub state: SessionState,
    /// Ociosa desde (só com `state` = `idle`).
    pub idle_since: Option<i64>,
}

/// O que o hub sabe de uma sub-issue do plano.
#[derive(Debug, Clone, Default)]
pub struct ItemSnap {
    /// Título no espelho; `None` = fora do espelho.
    pub title: Option<String>,
    /// Impressão digital agora (`ct_hub_store::issue_fingerprint`).
    pub fingerprint: Option<String>,
    /// PR aberto da sub-issue contra a `wip`, com os checks, o feedback e o
    /// estado do fechamento.
    pub open: Option<Closing>,
    /// PR da sub-issue mesclado na `wip`.
    pub merged: Option<i64>,
}

/// Fotografia de uma execução para uma decisão.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub now: i64,
    pub run: EpicRun,
    /// Versão aprovada em vigor; `None` = nenhuma ainda.
    pub plan: Option<EpicPlan>,
    pub nodes: Vec<EpicNode>,
    /// Branch de integração do épico (`wip/<épico>`).
    pub wip: String,
    pub items: BTreeMap<i64, ItemSnap>,
    /// Bloqueantes fora do épico: `true` = mesclado na base ou concluído.
    /// Ausente = desconhecido, bloqueia.
    pub external: BTreeMap<i64, bool>,
    /// Sessões dos nós, por id.
    pub sessions: BTreeMap<String, SessionSnap>,
    pub env_online: bool,
    /// Janela esgotada da conta do ambiente.
    pub exhausted: Option<Exhausted>,
    /// Pedidos de decisão pendentes (nó, origem).
    pub pending: HashSet<(Option<i64>, EpicDecisionOrigin)>,
    pub max_ci_attempts: u32,
    /// Prazo sem progresso (min); 0 = nunca.
    pub idle_timeout_min: u32,
}

/// Uma ação do ciclo. `message` vai para os eventos da execução.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Muda o status da execução.
    Status {
        to: EpicRunStatus,
        reason: Option<String>,
        resume_at: Option<i64>,
        message: String,
    },
    /// Adia a retomada da execução estacionada.
    Reschedule { resume_at: i64, message: String },
    /// Muda o estado do nó; com `pr`, grava o PR.
    Node {
        number: i64,
        to: EpicNodeState,
        pr: Option<i64>,
        reason: Option<String>,
        message: String,
    },
    /// Inicia a sessão do nó (o runner grava `em_execucao` e a sessão).
    Start {
        number: i64,
        title: String,
        model: String,
        effort: Option<String>,
        message: String,
    },
    /// Volta o nó estacionado ao estado de antes e retoma a sessão.
    Unpark {
        number: i64,
        session_id: String,
        via: ResumeVia,
        message: String,
    },
    /// Texto do fechamento à sessão (`session.continue`) e o estado novo.
    Send {
        number: i64,
        session_id: String,
        kind: SendKind,
        close: CloseState,
        text: String,
        message: String,
    },
    /// Grava o estado do fechamento sem enviar.
    Track { number: i64, close: CloseState },
    /// Pedido de rebase: com `session_id`, envia o texto agora e grava
    /// `rebase_at`; sem, só grava o pedido pendente (`rebase_head`).
    Rebase {
        number: i64,
        session_id: Option<String>,
        head: String,
        text: String,
        message: String,
    },
    /// O PR andou depois do pedido: limpa o rebase pendente.
    RebaseDone { number: i64 },
    /// Encerra a sessão do nó mesclado.
    End {
        number: i64,
        session_id: String,
        message: String,
    },
    /// Remove o worktree do nó mesclado (o runner limpa o `worktree` do nó
    /// com qualquer resultado; recusado, o motivo fica no nó).
    RemoveWorktree {
        number: i64,
        worktree: String,
        message: String,
    },
    /// Pedido de decisão ao desenvolvedor; com `block`, o nó fica
    /// `bloqueada`.
    Ask {
        number: Option<i64>,
        origin: EpicDecisionOrigin,
        block: bool,
        text: String,
    },
}

/// O resultado de um ciclo.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    pub actions: Vec<Action>,
    /// Nós em ciclo no grafo do plano (nada inicia).
    pub cycle: BTreeSet<i64>,
    /// Por que nada inicia agora (`paused`, `parked`, `exhausted`,
    /// `env_offline`, `cycle`, `full`, `resuming`, `status`); `None` = pode
    /// iniciar (ou iniciou).
    pub blocked: Option<&'static str>,
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// O status para onde a execução volta depois de pausada ou estacionada.
fn back_status(all_merged: bool) -> EpicRunStatus {
    if all_merged {
        EpicRunStatus::Integrando
    } else {
        EpicRunStatus::Executando
    }
}

/// Decide o ciclo de uma execução.
pub fn decide(s: &Snapshot) -> Outcome {
    let mut out = Outcome::default();
    let now = s.now;
    let run = &s.run;
    let Some(plan) = s.plan.as_ref() else {
        out.blocked = Some("status");
        return out;
    };
    if run.status.is_terminal() {
        out.blocked = Some("status");
        return out;
    }
    let slug = |n: i64| format!("{}/{}#{n}", run.owner, run.repo);
    let in_plan: BTreeSet<i64> = plan.nodes.iter().map(|n| n.number).collect();
    let nodes: BTreeMap<i64, &EpicNode> = s.nodes.iter().map(|n| (n.number, n)).collect();
    let exhausted = s.exhausted.as_ref().filter(|e| e.resets_at > now);

    // Ciclo no grafo do plano.
    let mut graph: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    for e in &plan.edges {
        graph.entry(e.number).or_default().insert(e.blocker);
    }
    out.cycle = cycle_members(&graph);

    // Merges na `wip` deste ciclo.
    let merged_now: Vec<i64> = s
        .nodes
        .iter()
        .filter(|n| n.state != EpicNodeState::Mesclada)
        .filter(|n| s.items.get(&n.number).is_some_and(|i| i.merged.is_some()))
        .map(|n| n.number)
        .collect();
    let merged = |n: i64| {
        nodes
            .get(&n)
            .is_some_and(|x| x.state == EpicNodeState::Mesclada)
            || merged_now.contains(&n)
    };
    let all_merged = !in_plan.is_empty() && in_plan.iter().all(|n| merged(*n));

    // 1. Orçamento: estacionar ou retomar a execução.
    let mut status = run.status;
    let mut resumed = false;
    if status.is_running() {
        if let Some(ex) = exhausted {
            let resume_at = ex.resets_at + RESUME_MARGIN_MS;
            out.actions.push(Action::Status {
                to: EpicRunStatus::Estacionado,
                reason: Some(ex.window.clone()),
                resume_at: Some(resume_at),
                message: format!(
                    "a janela {} da conta esgotou: a execução estacionou e retoma depois do \
                     reset",
                    ex.window
                ),
            });
            status = EpicRunStatus::Estacionado;
        }
    } else if status == EpicRunStatus::Estacionado {
        let due = run.resume_at.is_none_or(|at| now >= at);
        if due {
            if let Some(ex) = exhausted {
                let resume_at = ex.resets_at + RESUME_MARGIN_MS;
                if run.resume_at != Some(resume_at) {
                    out.actions.push(Action::Reschedule {
                        resume_at,
                        message: format!(
                            "a janela {} da conta segue esgotada: a retomada foi adiada",
                            ex.window
                        ),
                    });
                }
            } else {
                let to = back_status(all_merged);
                out.actions.push(Action::Status {
                    to,
                    reason: None,
                    resume_at: None,
                    message: "a janela da conta renovou: a execução foi retomada".into(),
                });
                status = to;
                resumed = true;
            }
        }
    }
    let running = status.is_running();
    // Plano em revisão (replanejar) com nós vivos da versão anterior: o
    // fechamento deles continua.
    let closing_on = running
        || (matches!(
            status,
            EpicRunStatus::AguardandoAprovacao | EpicRunStatus::Replanejando
        ) && exhausted.is_none());
    let holding = matches!(status, EpicRunStatus::Pausado | EpicRunStatus::Estacionado);

    let timeout_ms = i64::from(s.idle_timeout_min) * 60_000;
    let mut live: BTreeSet<i64> = BTreeSet::new();
    let mut became_ready: BTreeSet<i64> = BTreeSet::new();
    let mut asked: BTreeSet<i64> = BTreeSet::new();
    let pending_for = |n: i64| s.pending.iter().any(|(x, _)| *x == Some(n));

    // 2. Nós.
    let order: Vec<i64> = plan
        .nodes
        .iter()
        .map(|n| n.number)
        .chain(
            s.nodes
                .iter()
                .map(|n| n.number)
                .filter(|n| !in_plan.contains(n)),
        )
        .collect();
    for number in order {
        let Some(node) = nodes.get(&number).copied() else {
            continue;
        };
        let item = s.items.get(&number);
        let sess = node
            .session_id
            .as_deref()
            .and_then(|id| s.sessions.get(id).map(|x| (id, x)));
        let session_live = sess.is_some_and(|(_, x)| x.state.is_live());
        let idle_since = sess.and_then(|(_, x)| {
            (x.state == SessionState::Idle)
                .then_some(x.idle_since)
                .flatten()
        });

        // Sub-issue alterada depois do plano.
        let planned_fp = plan
            .nodes
            .iter()
            .find(|n| n.number == number)
            .and_then(|n| n.fingerprint.as_deref());
        if let (Some(f), Some(g)) = (planned_fp, item.and_then(|i| i.fingerprint.as_deref())) {
            if f != g
                && node.state != EpicNodeState::Mesclada
                && !s
                    .pending
                    .contains(&(Some(number), EpicDecisionOrigin::IssueAlterada))
            {
                out.actions.push(Action::Ask {
                    number: Some(number),
                    origin: EpicDecisionOrigin::IssueAlterada,
                    block: false,
                    text: format!(
                        "{} mudou depois da aprovação do plano (título, corpo, rótulos ou \
                         prioridade). Manter o plano ou replanejar?",
                        slug(number)
                    ),
                });
                asked.insert(number);
            }
        }

        // Merge na `wip`.
        if merged_now.contains(&number) {
            let pr = item.and_then(|i| i.merged);
            out.actions.push(Action::Node {
                number,
                to: EpicNodeState::Mesclada,
                pr,
                reason: None,
                message: format!(
                    "{}: PR #{} mesclado na {}",
                    slug(number),
                    pr.unwrap_or_default(),
                    s.wip
                ),
            });
            if node.state != EpicNodeState::Assumida {
                if let Some((id, _)) = sess.filter(|_| session_live) {
                    out.actions.push(Action::End {
                        number,
                        session_id: id.to_string(),
                        message: format!(
                            "{}: a sessão foi encerrada depois do merge",
                            slug(number)
                        ),
                    });
                }
            }
            continue;
        }

        match node.state {
            EpicNodeState::Mesclada => {
                if let Some(wt) = &node.worktree {
                    if !session_live {
                        out.actions.push(Action::RemoveWorktree {
                            number,
                            worktree: wt.clone(),
                            message: format!(
                                "{}: remoção do worktree depois do merge",
                                slug(number)
                            ),
                        });
                    }
                }
            }
            EpicNodeState::Assumida | EpicNodeState::Bloqueada => {
                if node.state == EpicNodeState::Assumida {
                    live.insert(number);
                }
            }
            EpicNodeState::Estacionada => {
                live.insert(number);
                if running {
                    let via = if session_live {
                        ResumeVia::Continue
                    } else {
                        ResumeVia::Resume
                    };
                    if let Some(id) = &node.session_id {
                        out.actions.push(Action::Unpark {
                            number,
                            session_id: id.clone(),
                            via,
                            message: format!("{}: a sessão foi retomada", slug(number)),
                        });
                    }
                }
            }
            EpicNodeState::EmExecucao | EpicNodeState::EmPr => {
                live.insert(number);
                if holding {
                    if idle_since.is_some() {
                        out.actions.push(Action::Node {
                            number,
                            to: EpicNodeState::Estacionada,
                            pr: None,
                            reason: None,
                            message: format!(
                                "{}: a sessão terminou o turno e estacionou",
                                slug(number)
                            ),
                        });
                    }
                    continue;
                }
                if !closing_on {
                    continue;
                }
                out.actions.extend(node_step(
                    s,
                    node,
                    item,
                    sess,
                    idle_since,
                    timeout_ms,
                    &merged_now,
                ));
            }
            EpicNodeState::Pendente | EpicNodeState::Pronta => {
                if !in_plan.contains(&number) {
                    continue;
                }
                let blockers: Vec<i64> = plan
                    .edges
                    .iter()
                    .filter(|e| e.number == number)
                    .map(|e| e.blocker)
                    .collect();
                let ready = blockers.iter().all(|b| {
                    if in_plan.contains(b) {
                        merged(*b)
                    } else {
                        s.external.get(b).copied().unwrap_or(false)
                    }
                });
                match (node.state, ready) {
                    (EpicNodeState::Pendente, true) => {
                        out.actions.push(Action::Node {
                            number,
                            to: EpicNodeState::Pronta,
                            pr: None,
                            reason: None,
                            message: format!("{}: pronta para iniciar", slug(number)),
                        });
                        became_ready.insert(number);
                    }
                    (EpicNodeState::Pronta, false) => {
                        out.actions.push(Action::Node {
                            number,
                            to: EpicNodeState::Pendente,
                            pr: None,
                            reason: None,
                            message: format!("{}: voltou a esperar um bloqueante", slug(number)),
                        });
                    }
                    (EpicNodeState::Pronta, true) => {
                        became_ready.insert(number);
                    }
                    _ => {}
                }
            }
        }
    }

    // 3. Início.
    let blocked = if status == EpicRunStatus::Pausado {
        Some("paused")
    } else if status == EpicRunStatus::Estacionado {
        Some("parked")
    } else if status != EpicRunStatus::Executando {
        Some("status")
    } else if resumed {
        Some("resuming")
    } else if exhausted.is_some() {
        Some("exhausted")
    } else if !s.env_online {
        Some("env_offline")
    } else if !out.cycle.is_empty() {
        Some("cycle")
    } else {
        None
    };
    out.blocked = blocked;
    if blocked.is_none() {
        let areas = |n: i64| -> &[String] {
            plan.nodes
                .iter()
                .find(|x| x.number == n)
                .map(|x| x.areas.as_slice())
                .unwrap_or(&[])
        };
        let mut occupied: Vec<i64> = live.iter().copied().collect();
        let max = s.run.max_parallel as usize;
        for pn in &plan.nodes {
            let n = pn.number;
            if !became_ready.contains(&n) || pending_for(n) || asked.contains(&n) {
                continue;
            }
            if occupied.len() >= max {
                out.blocked = Some("full");
                break;
            }
            if occupied.iter().any(|o| areas_overlap(areas(n), areas(*o))) {
                continue;
            }
            let (Some(model), Some(title)) = (
                pn.model.clone(),
                s.items.get(&n).and_then(|i| i.title.clone()),
            ) else {
                continue;
            };
            out.actions.push(Action::Start {
                number: n,
                title,
                model,
                effort: pn.effort.clone(),
                message: format!(
                    "{}: sessão iniciada ({}{})",
                    slug(n),
                    pn.model.as_deref().unwrap_or_default(),
                    pn.effort
                        .as_deref()
                        .map(|e| format!(", esforço {e}"))
                        .unwrap_or_default()
                ),
            });
            occupied.push(n);
        }
    }

    // 4. Integração.
    if status == EpicRunStatus::Executando && all_merged {
        out.actions.push(Action::Status {
            to: EpicRunStatus::Integrando,
            reason: None,
            resume_at: None,
            message: "todas as sub-issues foram mescladas na wip: começa a integração".into(),
        });
    }
    out
}

/// Um nó com sessão (`em_execucao` ou `em_pr`) na execução que roda: PR
/// aberto, rebase, fechamento e falta de progresso.
fn node_step(
    s: &Snapshot,
    node: &EpicNode,
    item: Option<&ItemSnap>,
    sess: Option<(&str, &SessionSnap)>,
    idle_since: Option<i64>,
    timeout_ms: i64,
    merged_now: &[i64],
) -> Vec<Action> {
    let now = s.now;
    let number = node.number;
    let slug = format!("{}/{}#{number}", s.run.owner, s.run.repo);
    let mut out = Vec::new();
    let open = item.and_then(|i| i.open.as_ref());
    let session_live = sess.is_some_and(|(_, x)| x.state.is_live());

    // Sessão sumida (ou que não apareceu depois do início).
    if !session_live {
        let known = sess.is_some();
        let due = known || now - node.updated_at >= START_GRACE_MS;
        if due
            && !s
                .pending
                .contains(&(Some(number), EpicDecisionOrigin::SemProgresso))
        {
            out.push(Action::Ask {
                number: Some(number),
                origin: EpicDecisionOrigin::SemProgresso,
                block: true,
                text: format!(
                    "{slug}: a sessão da sub-issue {} (sem PR mesclado). Assumir, reiniciar a \
                     sessão ou replanejar?",
                    if known {
                        "terminou"
                    } else {
                        "não apareceu depois do início"
                    }
                ),
            });
        }
        return out;
    }
    let (session_id, _) = sess.expect("viva");

    // PR aberto na `wip`.
    let Some(c) = open else {
        if node.state == EpicNodeState::EmPr {
            out.push(Action::Node {
                number,
                to: EpicNodeState::EmExecucao,
                pr: None,
                reason: Some("PR fechado sem merge".into()),
                message: format!("{slug}: o PR foi fechado sem merge"),
            });
            return out;
        }
        if let Some(since) = idle_since {
            if timeout_ms > 0 && now - since >= timeout_ms {
                let min = timeout_ms / 60_000;
                out.push(Action::Ask {
                    number: Some(number),
                    origin: EpicDecisionOrigin::SemProgresso,
                    block: true,
                    text: format!(
                        "{slug}: a sessão está ociosa há mais de {min} min sem abrir PR na {}. \
                         Assumir, reiniciar a sessão ou replanejar?",
                        s.wip
                    ),
                });
            }
        }
        return out;
    };
    if node.state == EpicNodeState::EmExecucao {
        out.push(Action::Node {
            number,
            to: EpicNodeState::EmPr,
            pr: Some(c.pr.number),
            reason: None,
            message: format!("{slug}: PR #{} aberto na {}", c.pr.number, s.wip),
        });
    }
    let head = c.pr.head_sha.clone();

    // Rebase.
    if let Some(rh) = &node.rebase_head {
        if head.as_deref() != Some(rh.as_str()) {
            out.push(Action::RebaseDone { number });
        } else if node.rebase_at.is_none() {
            if idle_since.is_some() {
                out.push(Action::Rebase {
                    number,
                    session_id: Some(session_id.to_string()),
                    head: rh.clone(),
                    text: rebase_text(c.pr.number, &s.wip, merged_now),
                    message: format!(
                        "{slug}: o pedido de rebase do PR #{} foi enviado",
                        c.pr.number
                    ),
                });
            }
            return out;
        } else {
            let sent = node.rebase_at.unwrap_or(now);
            if let Some(since) = idle_since {
                if timeout_ms > 0 && now - since.max(sent) >= timeout_ms {
                    out.push(Action::Ask {
                        number: Some(number),
                        origin: EpicDecisionOrigin::ConflitoRebase,
                        block: true,
                        text: format!(
                            "{slug}: o rebase do PR #{} na {} foi pedido e o PR não mudou \
                             (head {}). Assumir ou replanejar?",
                            c.pr.number,
                            s.wip,
                            short(rh)
                        ),
                    });
                }
            }
            return out;
        }
    } else if !merged_now.is_empty() {
        if let Some(h) = head.clone() {
            let send = idle_since.is_some();
            out.push(Action::Rebase {
                number,
                session_id: send.then(|| session_id.to_string()),
                head: h,
                text: rebase_text(c.pr.number, &s.wip, merged_now),
                message: format!(
                    "{slug}: rebase do PR #{} na {} {}",
                    c.pr.number,
                    s.wip,
                    if send {
                        "pedido à sessão"
                    } else {
                        "pendente (sessão ocupada)"
                    }
                ),
            });
            return out;
        }
    }

    // Fechamento (#184), com a sessão ociosa.
    let Some(since) = idle_since else {
        return out;
    };
    let step = close_decision(&CloseInput {
        now,
        max_ci_attempts: s.max_ci_attempts,
        idle_timeout_min: s.idle_timeout_min,
        idle_since: since,
        closing: c,
        slug: &slug,
        texts: EPIC_TEXTS,
    });
    match step {
        CloseStep::Wait => {}
        CloseStep::Track(close) => out.push(Action::Track { number, close }),
        CloseStep::Send {
            kind,
            close,
            text,
            message,
        } => out.push(Action::Send {
            number,
            session_id: session_id.to_string(),
            kind,
            close,
            text,
            message,
        }),
        CloseStep::Block {
            reason,
            status_reason,
            ..
        } => {
            let origin = if reason == BLOCK_ATTEMPTS {
                EpicDecisionOrigin::CiEsgotado
            } else {
                EpicDecisionOrigin::SemProgresso
            };
            let options = if origin == EpicDecisionOrigin::CiEsgotado {
                "Assumir, dar mais tentativas ou replanejar?"
            } else {
                "Assumir, reiniciar a sessão ou replanejar?"
            };
            let why = status_reason
                .strip_prefix("piloto automático: ")
                .unwrap_or(&status_reason);
            out.push(Action::Ask {
                number: Some(number),
                origin,
                block: true,
                text: format!("{slug}: {why}. {options}"),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests;
