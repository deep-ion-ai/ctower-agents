//! Execução de épico (#296, épico #295): estado persistente do motor que
//! executa as sub-issues de um épico a partir de um plano aprovado
//! (migração 46). A decisão fica em `cthubd::epic_run`; aqui só se guarda.
//!
//! - **Execução** ([`EpicRun`]): uma ativa por épico (índice único parcial),
//!   várias por ambiente, cada uma com o seu paralelismo.
//! - **Plano** ([`EpicPlan`]): versionado. A sessão de plano grava um
//!   rascunho ([`Hub::save_epic_plan_draft`]); a aprovação
//!   ([`Hub::approve_epic_plan`]) congela a versão, inclusive as arestas, que
//!   no rascunho são as vivas de `autopilot_dependency` (análise e
//!   correções manuais). A execução guarda a versão em vigor.
//! - **Nó** ([`EpicNode`]): o estado de cada sub-issue na execução. `assumida`
//!   e `estacionada` guardam o estado anterior para voltar a ele.
//! - **Pedidos de decisão** ([`EpicDecision`]) e **eventos**
//!   ([`EpicRunEvent`]) do motor.

use super::Hub;
use crate::model::IssueAgent;
use crate::BlockerInput;
use crate::{LedgerError, Result};
use ct_core::{
    normalize_areas, EpicDecisionOrigin, EpicNodeState, EpicPlanState, EpicRunStatus,
    EPIC_RUN_PARALLEL_CEILING, EPIC_RUN_PARALLEL_DEFAULT,
};
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Sub-issues por plano.
pub const EPIC_PLAN_MAX_NODES: usize = 200;
/// Propostas de sub-issue nova ou dividida por plano.
pub const EPIC_PLAN_MAX_PROPOSALS: usize = 50;
/// Justificativa de um nó, título de proposta: no máximo isto.
const SHORT_MAX_CHARS: usize = 500;
/// Resumo do plano, corpo de proposta, texto de pedido: no máximo isto.
const LONG_MAX_CHARS: usize = 4000;

/// Uma execução de épico.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EpicRun {
    pub id: i64,
    pub project_id: i64,
    pub owner: String,
    pub repo: String,
    /// Número do épico no rastreador.
    pub epic: i64,
    /// Ambiente onde as sessões rodam.
    pub env_id: String,
    pub status: EpicRunStatus,
    /// Versão do plano aprovado em vigor; `None` = nenhuma aprovada ainda.
    pub plan_version: Option<i64>,
    /// Sessões de sub-issue vivas ao mesmo tempo.
    pub max_parallel: u32,
    /// A sessão de integração roda o `/code-review`.
    pub code_review: bool,
    /// Quem disparou a execução.
    pub started_by: String,
    /// Sessão de plano em andamento (a única que grava o rascunho); `None`
    /// = nenhuma.
    pub plan_session: Option<String>,
    /// Modelo e esforço da sessão de plano; `None` = o padrão do hub.
    #[serde(default)]
    pub plan_model: Option<String>,
    #[serde(default)]
    pub plan_effort: Option<String>,
    /// Sessão de integração em andamento (#302); `None` = nenhuma.
    #[serde(default)]
    pub integration_session: Option<String>,
    /// Motivo da pausa ou do estacionamento.
    pub paused_reason: Option<String>,
    /// Estacionada: quando o motor retoma (ms).
    pub resume_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Concluída ou cancelada em.
    pub ended_at: Option<i64>,
}

/// O que é preciso para criar uma execução ([`Hub::create_epic_run`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEpicRun {
    pub project_id: i64,
    pub epic: i64,
    pub env_id: String,
    pub started_by: String,
    /// `None` = [`EPIC_RUN_PARALLEL_DEFAULT`].
    pub max_parallel: Option<u32>,
    /// Modelo e esforço da sessão de plano; `None` = o padrão do hub.
    pub plan_model: Option<String>,
    pub plan_effort: Option<String>,
}

/// Uma sub-issue no plano.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EpicPlanNode {
    pub number: i64,
    /// Ordem sugerida (0 = primeira).
    pub position: i64,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Caminhos ou globs que a sub-issue toca, normalizados
    /// ([`ct_core::normalize_area`]).
    pub areas: Vec<String>,
    /// Justificativa curta.
    pub reason: String,
    /// Impressão digital da sub-issue na aprovação
    /// ([`crate::issue_fingerprint`]); `None` no rascunho.
    pub fingerprint: Option<String>,
}

/// Aresta do plano: `number` é bloqueada por `blocker`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EpicPlanEdge {
    pub number: i64,
    pub blocker: i64,
    pub reason: String,
    /// `analysis` ou `manual`.
    pub source: String,
}

/// Proposta da sessão de plano: sub-issue nova (`nova`) ou divisão de uma
/// existente (`dividir`, com `from`). Só é criada depois da aprovação.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EpicProposal {
    pub kind: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub from: Option<i64>,
}

/// Uma versão do plano.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EpicPlan {
    pub run_id: i64,
    pub version: i64,
    pub state: EpicPlanState,
    pub summary: String,
    /// Nós na ordem sugerida.
    pub nodes: Vec<EpicPlanNode>,
    /// Rascunho: as arestas vivas do projeto entre os nós; aprovado ou
    /// substituído: as congeladas na aprovação.
    pub edges: Vec<EpicPlanEdge>,
    pub proposals: Vec<EpicProposal>,
    /// Sessão de plano que gravou a versão.
    pub session_id: Option<String>,
    pub created_at: i64,
    pub approved_at: Option<i64>,
    pub approved_by: Option<String>,
}

/// Um nó do plano como a sessão de plano o grava.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EpicPlanNodeInput {
    pub number: i64,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub areas: Vec<String>,
    #[serde(default)]
    pub reason: String,
}

/// Rascunho do plano ([`Hub::save_epic_plan_draft`]). A ordem de `nodes` é
/// a ordem sugerida.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EpicPlanDraft {
    pub nodes: Vec<EpicPlanNodeInput>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub proposals: Vec<EpicProposal>,
    #[serde(default)]
    pub session_id: Option<String>,
}

/// Uma sub-issue do plano como a sessão de plano a grava (#299): o nó e as
/// issues que a bloqueiam.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EpicPlanSessionNode {
    #[serde(flatten)]
    pub node: EpicPlanNodeInput,
    #[serde(default)]
    pub blocked_by: Vec<BlockerInput>,
}

/// O plano da sessão de plano (`epic_set_plan`, #299).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EpicPlanSessionInput {
    pub nodes: Vec<EpicPlanSessionNode>,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub proposals: Vec<EpicProposal>,
}

/// A aprovação de uma versão ([`Hub::approve_epic_plan`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicApproval {
    /// A versão vista pela pessoa: precisa ser o rascunho mais recente.
    pub version: i64,
    pub approved_by: String,
    pub max_parallel: u32,
    pub code_review: bool,
    /// Impressão digital de cada sub-issue agora, congelada com o plano
    /// (as que faltarem ficam sem).
    pub fingerprints: BTreeMap<i64, String>,
}

/// O estado de uma sub-issue na execução.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EpicNode {
    pub run_id: i64,
    pub number: i64,
    pub state: EpicNodeState,
    /// Em `assumida` ou `estacionada`: o estado para onde volta.
    pub prev_state: Option<EpicNodeState>,
    pub session_id: Option<String>,
    pub worktree: Option<String>,
    pub pr: Option<i64>,
    /// Tentativas de correção do CI já pedidas.
    pub attempts: u32,
    /// Desde quando a pessoa assumiu a sessão.
    pub assumed_since: Option<i64>,
    /// Motivo do estado (bloqueio, remoção do worktree recusada...).
    pub reason: Option<String>,
    /// Head do PR quando o motor pediu o rebase (depois de um merge na
    /// `wip`); `None` = nenhum pedido pendente.
    pub rebase_head: Option<String>,
    /// Quando o pedido de rebase foi enviado à sessão; `None` com
    /// `rebase_head` = ainda por enviar.
    pub rebase_at: Option<i64>,
    pub updated_at: i64,
}

/// Mudança nos campos do nó além do estado. `None` = não mexe; `Some(None)`
/// = limpa.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EpicNodePatch {
    pub session_id: Option<Option<String>>,
    pub worktree: Option<Option<String>>,
    pub pr: Option<Option<i64>>,
    pub attempts: Option<u32>,
    pub reason: Option<Option<String>>,
    pub rebase_head: Option<Option<String>>,
    pub rebase_at: Option<Option<i64>>,
}

/// A sessão de um nó ([`Hub::epic_session_info`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicSessionInfo {
    pub state: ct_core::SessionState,
    /// Ociosa desde (vale com `state` = `idle`).
    pub idle_since: i64,
    pub cwd: String,
}

/// Um pedido de decisão ao desenvolvedor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EpicDecision {
    pub id: i64,
    pub run_id: i64,
    /// Sub-issue do pedido; `None` = da execução inteira.
    pub number: Option<i64>,
    pub origin: EpicDecisionOrigin,
    pub text: String,
    pub options: Vec<String>,
    /// `None` = pendente.
    pub answer: Option<String>,
    pub created_at: i64,
    pub answered_at: Option<i64>,
    pub answered_by: Option<String>,
}

/// Um evento do motor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EpicRunEvent {
    /// Ignorado ao gravar.
    pub id: i64,
    pub run_id: i64,
    pub at: i64,
    pub kind: String,
    pub number: Option<i64>,
    /// Código estável do motivo.
    pub reason: Option<String>,
    /// Texto para a pessoa (pt-BR).
    pub message: String,
}

fn clip(s: &str, max: usize) -> String {
    let t = s.trim();
    match t.char_indices().nth(max) {
        Some((i, _)) => t[..i].to_string(),
        None => t.to_string(),
    }
}

fn enum_col<T: std::str::FromStr>(r: &Row, i: usize) -> rusqlite::Result<T>
where
    T::Err: std::fmt::Display,
{
    let s: String = r.get(i)?;
    s.parse().map_err(|e: T::Err| {
        rusqlite::Error::FromSqlConversionFailure(
            i,
            rusqlite::types::Type::Text,
            e.to_string().into(),
        )
    })
}

fn json_vec<T: serde::de::DeserializeOwned>(r: &Row, i: usize) -> rusqlite::Result<Vec<T>> {
    let s: String = r.get(i)?;
    Ok(serde_json::from_str(&s).unwrap_or_default())
}

const RUN_COLS: &str = "r.id, r.project_id, p.gh_owner, p.gh_repo, r.epic, r.env_id, r.status, \
                        r.plan_version, r.max_parallel, r.code_review, r.started_by, \
                        r.plan_session, r.paused_reason, r.resume_at, r.created_at, \
                        r.updated_at, r.ended_at, r.plan_model, r.plan_effort, \
                        r.integration_session";
const RUN_FROM: &str = "epic_run r JOIN project p ON p.id = r.project_id";
const NODE_COLS: &str = "run_id, number, state, prev_state, session_id, worktree, pr, attempts, \
                         assumed_since, reason, rebase_head, rebase_at, updated_at";
const DECISION_COLS: &str = "id, run_id, number, origin, text, options, answer, created_at, \
                             answered_at, answered_by";
const ACTIVE: &str = "status NOT IN ('concluido', 'cancelado')";

fn row_to_run(r: &Row) -> rusqlite::Result<EpicRun> {
    Ok(EpicRun {
        id: r.get(0)?,
        project_id: r.get(1)?,
        owner: r.get(2)?,
        repo: r.get(3)?,
        epic: r.get(4)?,
        env_id: r.get(5)?,
        status: enum_col(r, 6)?,
        plan_version: r.get(7)?,
        max_parallel: r.get::<_, i64>(8)?.clamp(1, u32::MAX as i64) as u32,
        code_review: r.get(9)?,
        started_by: r.get(10)?,
        plan_session: r.get(11)?,
        paused_reason: r.get(12)?,
        resume_at: r.get(13)?,
        created_at: r.get(14)?,
        updated_at: r.get(15)?,
        ended_at: r.get(16)?,
        plan_model: r.get(17)?,
        plan_effort: r.get(18)?,
        integration_session: r.get(19)?,
    })
}

fn row_to_node(r: &Row) -> rusqlite::Result<EpicNode> {
    let prev: Option<String> = r.get(3)?;
    Ok(EpicNode {
        run_id: r.get(0)?,
        number: r.get(1)?,
        state: enum_col(r, 2)?,
        prev_state: prev.and_then(|s| s.parse().ok()),
        session_id: r.get(4)?,
        worktree: r.get(5)?,
        pr: r.get(6)?,
        attempts: r.get::<_, i64>(7)?.clamp(0, u32::MAX as i64) as u32,
        assumed_since: r.get(8)?,
        reason: r.get(9)?,
        rebase_head: r.get(10)?,
        rebase_at: r.get(11)?,
        updated_at: r.get(12)?,
    })
}

fn row_to_decision(r: &Row) -> rusqlite::Result<EpicDecision> {
    Ok(EpicDecision {
        id: r.get(0)?,
        run_id: r.get(1)?,
        number: r.get(2)?,
        origin: enum_col(r, 3)?,
        text: r.get(4)?,
        options: json_vec(r, 5)?,
        answer: r.get(6)?,
        created_at: r.get(7)?,
        answered_at: r.get(8)?,
        answered_by: r.get(9)?,
    })
}

fn check_parallel(n: u32) -> Result<()> {
    if !(1..=EPIC_RUN_PARALLEL_CEILING).contains(&n) {
        return Err(LedgerError::Invalid(format!(
            "paralelismo da execução: de 1 a {EPIC_RUN_PARALLEL_CEILING}"
        )));
    }
    Ok(())
}

/// Confere e normaliza o rascunho: nós únicos e positivos (sem o próprio
/// épico), modelo e esforço aceitos, áreas normalizadas, propostas bem
/// formadas. Os textos são cortados.
fn check_draft(epic: i64, d: &EpicPlanDraft) -> Result<(Vec<EpicPlanNode>, Vec<EpicProposal>)> {
    if d.nodes.is_empty() {
        return Err(LedgerError::Invalid("plano sem sub-issues".into()));
    }
    if d.nodes.len() > EPIC_PLAN_MAX_NODES {
        return Err(LedgerError::Invalid(format!(
            "no máximo {EPIC_PLAN_MAX_NODES} sub-issues por plano"
        )));
    }
    let mut seen = BTreeSet::new();
    let mut nodes = Vec::with_capacity(d.nodes.len());
    for (i, n) in d.nodes.iter().enumerate() {
        if n.number <= 0 || n.number == epic {
            return Err(LedgerError::Invalid(format!(
                "sub-issue inválida no plano: {}",
                n.number
            )));
        }
        if !seen.insert(n.number) {
            return Err(LedgerError::Invalid(format!(
                "sub-issue #{} repetida no plano",
                n.number
            )));
        }
        let agent = IssueAgent {
            model: n.model.clone(),
            effort: n.effort.clone(),
        };
        agent
            .validate()
            .map_err(|e| LedgerError::Invalid(format!("sub-issue #{}: {e}", n.number)))?;
        let areas = normalize_areas(&n.areas)
            .map_err(|e| LedgerError::Invalid(format!("sub-issue #{}: {e}", n.number)))?;
        nodes.push(EpicPlanNode {
            number: n.number,
            position: i as i64,
            model: n.model.clone(),
            effort: n.effort.clone(),
            areas,
            reason: clip(&n.reason, SHORT_MAX_CHARS),
            fingerprint: None,
        });
    }
    if d.proposals.len() > EPIC_PLAN_MAX_PROPOSALS {
        return Err(LedgerError::Invalid(format!(
            "no máximo {EPIC_PLAN_MAX_PROPOSALS} propostas por plano"
        )));
    }
    let mut proposals = Vec::with_capacity(d.proposals.len());
    for p in &d.proposals {
        let title = clip(&p.title, SHORT_MAX_CHARS);
        if title.is_empty() {
            return Err(LedgerError::Invalid("proposta sem título".into()));
        }
        match (p.kind.as_str(), p.from) {
            ("nova", _) => {}
            ("dividir", Some(f)) if f > 0 => {}
            ("dividir", _) => {
                return Err(LedgerError::Invalid(format!(
                    "proposta \"{title}\": dividir precisa de from (a sub-issue dividida)"
                )))
            }
            (k, _) => {
                return Err(LedgerError::Invalid(format!(
                    "proposta \"{title}\": tipo inválido {k} (nova ou dividir)"
                )))
            }
        }
        proposals.push(EpicProposal {
            kind: p.kind.clone(),
            title,
            body: clip(&p.body, LONG_MAX_CHARS),
            from: p.from,
        });
    }
    Ok((nodes, proposals))
}

impl Hub {
    // --- execução --------------------------------------------------------

    /// Cria a execução em `planejando`. Já há uma ativa para o épico =
    /// `Conflict`.
    pub fn create_epic_run(&self, n: &NewEpicRun, now: i64) -> Result<EpicRun> {
        if n.epic <= 0 {
            return Err(LedgerError::Invalid(format!("épico inválido: {}", n.epic)));
        }
        if n.env_id.trim().is_empty() {
            return Err(LedgerError::Invalid("execução sem ambiente".into()));
        }
        let parallel = n.max_parallel.unwrap_or(EPIC_RUN_PARALLEL_DEFAULT);
        check_parallel(parallel)?;
        IssueAgent {
            model: n.plan_model.clone(),
            effort: n.plan_effort.clone(),
        }
        .validate()
        .map_err(|e| LedgerError::Invalid(format!("sessão de plano: {e}")))?;
        let live: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM project WHERE id = ?1 AND deleted_at IS NULL",
                [n.project_id],
                |r| r.get(0),
            )
            .optional()?;
        if live.is_none() {
            return Err(LedgerError::NotFound(
                "projeto",
                format!("id {}", n.project_id),
            ));
        }
        if let Some(r) = self.active_epic_run(n.project_id, n.epic)? {
            return Err(LedgerError::Conflict(format!(
                "o épico #{} já tem uma execução ativa ({})",
                n.epic, r.status
            )));
        }
        self.conn.execute(
            "INSERT INTO epic_run (project_id, epic, env_id, status, max_parallel, started_by,
                                   created_at, updated_at, plan_model, plan_effort)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9)",
            params![
                n.project_id,
                n.epic,
                n.env_id,
                EpicRunStatus::Planejando.as_str(),
                parallel,
                n.started_by,
                now,
                n.plan_model,
                n.plan_effort
            ],
        )?;
        self.require_epic_run(self.conn.last_insert_rowid())
    }

    pub fn epic_run(&self, id: i64) -> Result<Option<EpicRun>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {RUN_COLS} FROM {RUN_FROM} WHERE r.id = ?1"),
                [id],
                row_to_run,
            )
            .optional()?)
    }

    fn require_epic_run(&self, id: i64) -> Result<EpicRun> {
        self.epic_run(id)?
            .ok_or_else(|| LedgerError::NotFound("execução de épico", id.to_string()))
    }

    /// A execução ativa (não concluída nem cancelada) do épico.
    pub fn active_epic_run(&self, project_id: i64, epic: i64) -> Result<Option<EpicRun>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {RUN_COLS} FROM {RUN_FROM}
                     WHERE r.project_id = ?1 AND r.epic = ?2 AND r.{ACTIVE}"
                ),
                params![project_id, epic],
                row_to_run,
            )
            .optional()?)
    }

    /// Todas as execuções ativas, de todos os ambientes, por id.
    pub fn active_epic_runs(&self) -> Result<Vec<EpicRun>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RUN_COLS} FROM {RUN_FROM} WHERE r.{ACTIVE} ORDER BY r.id"
        ))?;
        let rows = stmt.query_map([], row_to_run)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Sub-issues presas a uma execução ativa do projeto (#303): os nós da
    /// execução, as do plano mais recente e as filhas do épico no espelho.
    /// O piloto automático não as inicia.
    pub fn epic_run_numbers(&self, project_id: i64) -> Result<BTreeSet<i64>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT n.number FROM epic_node n JOIN epic_run r ON r.id = n.run_id
             WHERE r.project_id = ?1 AND r.{ACTIVE}
             UNION
             SELECT pn.number FROM epic_plan_node pn JOIN epic_run r ON r.id = pn.run_id
             WHERE r.project_id = ?1 AND r.{ACTIVE}
               AND pn.version = (SELECT max(version) FROM epic_plan WHERE run_id = r.id)
             UNION
             SELECT i.gh_number FROM issue_cache i JOIN epic_run r
               ON r.project_id = i.project_id AND i.parent_number = r.epic
             WHERE r.project_id = ?1 AND r.{ACTIVE}"
        ))?;
        let rows = stmt.query_map([project_id], |r| r.get::<_, i64>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// As execuções do épico, da mais nova para a mais antiga.
    pub fn epic_runs(&self, project_id: i64, epic: i64) -> Result<Vec<EpicRun>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RUN_COLS} FROM {RUN_FROM} WHERE r.project_id = ?1 AND r.epic = ?2
             ORDER BY r.id DESC"
        ))?;
        let rows = stmt.query_map(params![project_id, epic], row_to_run)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Muda o status da execução, recusando a transição inválida
    /// ([`EpicRunStatus::can_transition`]). Entrar em `pausado` grava o
    /// motivo; em `estacionado`, o motivo e o `resume_at` (obrigatório).
    /// Sair de um dos dois limpa os dois. Concluir ou cancelar grava o fim.
    pub fn set_epic_run_status(
        &self,
        id: i64,
        to: EpicRunStatus,
        reason: Option<&str>,
        resume_at: Option<i64>,
        now: i64,
    ) -> Result<EpicRun> {
        let tx = self.tx()?;
        let run = self.require_epic_run(id)?;
        if !run.status.can_transition(to) {
            return Err(LedgerError::Conflict(format!(
                "transição inválida da execução: {} -> {to}",
                run.status
            )));
        }
        let (reason, resume_at) = match to {
            EpicRunStatus::Pausado => (reason.map(|r| clip(r, SHORT_MAX_CHARS)), None),
            EpicRunStatus::Estacionado => {
                let Some(at) = resume_at else {
                    return Err(LedgerError::Invalid(
                        "estacionar a execução exige o resume_at".into(),
                    ));
                };
                (reason.map(|r| clip(r, SHORT_MAX_CHARS)), Some(at))
            }
            _ => (None, None),
        };
        self.conn.execute(
            "UPDATE epic_run SET status = ?2, paused_reason = ?3, resume_at = ?4,
                    updated_at = ?5, ended_at = CASE WHEN ?6 THEN ?5 ELSE ended_at END
             WHERE id = ?1",
            params![id, to.as_str(), reason, resume_at, now, to.is_terminal()],
        )?;
        tx.commit()?;
        self.require_epic_run(id)
    }

    /// Grava (ou limpa) a sessão de plano em andamento.
    pub fn set_epic_run_plan_session(
        &self,
        id: i64,
        session: Option<&str>,
        now: i64,
    ) -> Result<EpicRun> {
        self.require_epic_run(id)?;
        self.conn.execute(
            "UPDATE epic_run SET plan_session = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, session, now],
        )?;
        self.require_epic_run(id)
    }

    /// Grava (ou limpa) a sessão de integração em andamento (#302).
    pub fn set_epic_run_integration_session(
        &self,
        id: i64,
        session: Option<&str>,
        now: i64,
    ) -> Result<EpicRun> {
        self.require_epic_run(id)?;
        self.conn.execute(
            "UPDATE epic_run SET integration_session = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, session, now],
        )?;
        self.require_epic_run(id)
    }

    /// Adia a retomada da execução estacionada (a janela renovou e esgotou
    /// de novo).
    pub fn set_epic_run_resume_at(&self, id: i64, resume_at: i64, now: i64) -> Result<EpicRun> {
        let run = self.require_epic_run(id)?;
        if run.status != EpicRunStatus::Estacionado {
            return Err(LedgerError::Conflict(format!(
                "a execução não está estacionada ({})",
                run.status
            )));
        }
        self.conn.execute(
            "UPDATE epic_run SET resume_at = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, resume_at, now],
        )?;
        self.require_epic_run(id)
    }

    /// Grava a impressão digital atual da sub-issue na versão em vigor do
    /// plano ("manter o plano" depois de a sub-issue mudar).
    pub fn set_epic_plan_fingerprint(&self, run_id: i64, number: i64, fp: &str) -> Result<()> {
        let run = self.require_epic_run(run_id)?;
        let Some(v) = run.plan_version else {
            return Err(LedgerError::Conflict(
                "a execução não tem plano aprovado".into(),
            ));
        };
        let n = self.conn.execute(
            "UPDATE epic_plan_node SET fingerprint = ?4
             WHERE run_id = ?1 AND version = ?2 AND number = ?3",
            params![run_id, v, number, fp],
        )?;
        if n == 0 {
            return Err(LedgerError::NotFound(
                "sub-issue do plano",
                format!("#{number} na versão {v}"),
            ));
        }
        Ok(())
    }

    /// Muda o paralelismo (1 a [`EPIC_RUN_PARALLEL_CEILING`]) da execução
    /// não terminada.
    pub fn set_epic_run_parallel(&self, id: i64, n: u32, now: i64) -> Result<EpicRun> {
        check_parallel(n)?;
        let run = self.require_epic_run(id)?;
        if run.status.is_terminal() {
            return Err(LedgerError::Conflict(format!(
                "a execução já terminou ({})",
                run.status
            )));
        }
        self.conn.execute(
            "UPDATE epic_run SET max_parallel = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, n, now],
        )?;
        self.require_epic_run(id)
    }

    // --- plano -----------------------------------------------------------

    /// Grava o rascunho do plano da execução em `planejando`,
    /// `replanejando` ou `aguardando_aprovacao`. O rascunho anterior ainda
    /// não aprovado é substituído (mesma versão); depois de uma aprovação, a
    /// versão é a seguinte. A execução fica em `aguardando_aprovacao`.
    pub fn save_epic_plan_draft(
        &self,
        run_id: i64,
        draft: &EpicPlanDraft,
        now: i64,
    ) -> Result<EpicPlan> {
        let tx = self.tx()?;
        let run = self.require_epic_run(run_id)?;
        if !matches!(
            run.status,
            EpicRunStatus::Planejando
                | EpicRunStatus::Replanejando
                | EpicRunStatus::AguardandoAprovacao
        ) {
            return Err(LedgerError::Conflict(format!(
                "a execução não espera plano ({})",
                run.status
            )));
        }
        let (nodes, proposals) = check_draft(run.epic, draft)?;
        let last: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT version, state FROM epic_plan WHERE run_id = ?1
                 ORDER BY version DESC LIMIT 1",
                [run_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let version = match last {
            Some((v, s)) if s == EpicPlanState::Rascunho.as_str() => {
                self.conn.execute(
                    "DELETE FROM epic_plan WHERE run_id = ?1 AND version = ?2",
                    params![run_id, v],
                )?;
                v
            }
            Some((v, _)) => v + 1,
            None => 1,
        };
        self.conn.execute(
            "INSERT INTO epic_plan (run_id, version, state, summary, proposals, session_id,
                                    created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                run_id,
                version,
                EpicPlanState::Rascunho.as_str(),
                clip(&draft.summary, LONG_MAX_CHARS),
                serde_json::to_string(&proposals).unwrap_or_else(|_| "[]".into()),
                draft.session_id,
                now
            ],
        )?;
        for n in &nodes {
            self.conn.execute(
                "INSERT INTO epic_plan_node (run_id, version, number, position, model, effort,
                                             areas, reason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    run_id,
                    version,
                    n.number,
                    n.position,
                    n.model,
                    n.effort,
                    serde_json::to_string(&n.areas).unwrap_or_else(|_| "[]".into()),
                    n.reason
                ],
            )?;
        }
        if run.status != EpicRunStatus::AguardandoAprovacao {
            self.conn.execute(
                "UPDATE epic_run SET status = ?2, updated_at = ?3 WHERE id = ?1",
                params![run_id, EpicRunStatus::AguardandoAprovacao.as_str(), now],
            )?;
        } else {
            self.conn.execute(
                "UPDATE epic_run SET updated_at = ?2 WHERE id = ?1",
                params![run_id, now],
            )?;
        }
        tx.commit()?;
        self.epic_plan(run_id, version)?
            .ok_or_else(|| LedgerError::Corrupt("plano recém-gravado sumiu".into()))
    }

    /// Aprova o rascunho mais recente: congela as arestas ativas do projeto
    /// entre os nós e seus bloqueantes, marca a versão aprovada anterior como
    /// substituída, grava o paralelismo e o `/code-review` escolhidos, cria
    /// os nós novos (`pendente`) e leva a execução a `executando`. Os nós
    /// fora do novo plano que ainda não começaram saem; os demais ficam. Todo
    /// nó precisa de modelo.
    pub fn approve_epic_plan(&self, run_id: i64, a: &EpicApproval, now: i64) -> Result<EpicRun> {
        check_parallel(a.max_parallel)?;
        let tx = self.tx()?;
        let run = self.require_epic_run(run_id)?;
        if run.status != EpicRunStatus::AguardandoAprovacao {
            return Err(LedgerError::Conflict(format!(
                "a execução não aguarda aprovação ({})",
                run.status
            )));
        }
        let Some(plan) = self.latest_epic_plan(run_id)? else {
            return Err(LedgerError::Conflict(
                "a execução ainda não tem plano".into(),
            ));
        };
        if plan.state != EpicPlanState::Rascunho || plan.version != a.version {
            return Err(LedgerError::Conflict(format!(
                "o plano mudou: a versão a aprovar é a {} ({})",
                plan.version, plan.state
            )));
        }
        if let Some(n) = plan.nodes.iter().find(|n| n.model.is_none()) {
            return Err(LedgerError::Conflict(format!(
                "sub-issue #{} sem modelo: defina antes de aprovar",
                n.number
            )));
        }
        for e in &plan.edges {
            self.conn.execute(
                "INSERT INTO epic_plan_edge (run_id, version, number, blocker, reason, source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    run_id,
                    plan.version,
                    e.number,
                    e.blocker,
                    e.reason,
                    e.source
                ],
            )?;
        }
        for (n, f) in &a.fingerprints {
            self.conn.execute(
                "UPDATE epic_plan_node SET fingerprint = ?4
                 WHERE run_id = ?1 AND version = ?2 AND number = ?3",
                params![run_id, plan.version, n, f],
            )?;
        }
        self.conn.execute(
            "UPDATE epic_plan SET state = ?2 WHERE run_id = ?1 AND state = ?3",
            params![
                run_id,
                EpicPlanState::Substituido.as_str(),
                EpicPlanState::Aprovado.as_str()
            ],
        )?;
        self.conn.execute(
            "UPDATE epic_plan SET state = ?3, approved_at = ?4, approved_by = ?5
             WHERE run_id = ?1 AND version = ?2",
            params![
                run_id,
                plan.version,
                EpicPlanState::Aprovado.as_str(),
                now,
                a.approved_by
            ],
        )?;
        let numbers: BTreeSet<i64> = plan.nodes.iter().map(|n| n.number).collect();
        for node in self.epic_nodes(run_id)? {
            let idle = matches!(
                node.state,
                EpicNodeState::Pendente | EpicNodeState::Pronta | EpicNodeState::Bloqueada
            ) && node.session_id.is_none();
            if !numbers.contains(&node.number) && idle {
                self.conn.execute(
                    "DELETE FROM epic_node WHERE run_id = ?1 AND number = ?2",
                    params![run_id, node.number],
                )?;
            }
        }
        for n in &numbers {
            self.conn.execute(
                "INSERT INTO epic_node (run_id, number, state, updated_at)
                 VALUES (?1, ?2, ?3, ?4) ON CONFLICT (run_id, number) DO NOTHING",
                params![run_id, n, EpicNodeState::Pendente.as_str(), now],
            )?;
        }
        self.conn.execute(
            "UPDATE epic_run SET status = ?2, plan_version = ?3, max_parallel = ?4,
                    code_review = ?5, updated_at = ?6
             WHERE id = ?1",
            params![
                run_id,
                EpicRunStatus::Executando.as_str(),
                plan.version,
                a.max_parallel,
                a.code_review,
                now
            ],
        )?;
        tx.commit()?;
        self.require_epic_run(run_id)
    }

    /// Uma versão do plano.
    pub fn epic_plan(&self, run_id: i64, version: i64) -> Result<Option<EpicPlan>> {
        let head = self
            .conn
            .query_row(
                "SELECT pl.state, pl.summary, pl.proposals, pl.session_id, pl.created_at,
                        pl.approved_at, pl.approved_by, r.project_id
                 FROM epic_plan pl JOIN epic_run r ON r.id = pl.run_id
                 WHERE pl.run_id = ?1 AND pl.version = ?2",
                params![run_id, version],
                |r| {
                    Ok((
                        EpicPlan {
                            run_id,
                            version,
                            state: enum_col(r, 0)?,
                            summary: r.get(1)?,
                            nodes: vec![],
                            edges: vec![],
                            proposals: json_vec(r, 2)?,
                            session_id: r.get(3)?,
                            created_at: r.get(4)?,
                            approved_at: r.get(5)?,
                            approved_by: r.get(6)?,
                        },
                        r.get::<_, i64>(7)?,
                    ))
                },
            )
            .optional()?;
        let Some((mut plan, project_id)) = head else {
            return Ok(None);
        };
        let mut stmt = self.conn.prepare(
            "SELECT number, position, model, effort, areas, reason, fingerprint FROM epic_plan_node
             WHERE run_id = ?1 AND version = ?2 ORDER BY position, number",
        )?;
        plan.nodes = stmt
            .query_map(params![run_id, version], |r| {
                Ok(EpicPlanNode {
                    number: r.get(0)?,
                    position: r.get(1)?,
                    model: r.get(2)?,
                    effort: r.get(3)?,
                    areas: json_vec(r, 4)?,
                    reason: r.get(5)?,
                    fingerprint: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        plan.edges = if plan.state == EpicPlanState::Rascunho {
            let numbers: BTreeSet<i64> = plan.nodes.iter().map(|n| n.number).collect();
            self.autopilot_dependencies(project_id)?
                .into_iter()
                .filter(|e| !e.removed && numbers.contains(&e.number))
                .map(|e| EpicPlanEdge {
                    number: e.number,
                    blocker: e.blocker,
                    reason: e.reason,
                    source: e.source,
                })
                .collect()
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT number, blocker, reason, source FROM epic_plan_edge
                 WHERE run_id = ?1 AND version = ?2 ORDER BY number, blocker",
            )?;
            let rows = stmt.query_map(params![run_id, version], |r| {
                Ok(EpicPlanEdge {
                    number: r.get(0)?,
                    blocker: r.get(1)?,
                    reason: r.get(2)?,
                    source: r.get(3)?,
                })
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        Ok(Some(plan))
    }

    /// A versão mais recente do plano (rascunho ou aprovada).
    pub fn latest_epic_plan(&self, run_id: i64) -> Result<Option<EpicPlan>> {
        let v: Option<i64> = self.conn.query_row(
            "SELECT max(version) FROM epic_plan WHERE run_id = ?1",
            [run_id],
            |r| r.get(0),
        )?;
        match v {
            Some(v) => self.epic_plan(run_id, v),
            None => Ok(None),
        }
    }

    /// Sub-issues do épico no espelho do projeto (abertas ou fechadas).
    pub fn epic_sub_issues(&self, project_id: i64, epic: i64) -> Result<BTreeSet<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT gh_number FROM issue_cache WHERE project_id = ?1 AND parent_number = ?2",
        )?;
        let rows = stmt.query_map(params![project_id, epic], |r| r.get::<_, i64>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Grava o plano da sessão de plano (#299): recusa nó fora das
    /// sub-issues do épico; substitui as arestas de análise de todas as
    /// sub-issues do épico pelas informadas (as manuais ficam) e grava o
    /// rascunho ([`Hub::save_epic_plan_draft`]) com a sessão. A detecção de
    /// ciclo fica com quem chama (o `cthubd`, com o grafo efetivo).
    pub fn save_epic_plan_from_session(
        &self,
        run_id: i64,
        input: &EpicPlanSessionInput,
        session_id: &str,
        now: i64,
    ) -> Result<EpicPlan> {
        let run = self.require_epic_run(run_id)?;
        let subs = self.epic_sub_issues(run.project_id, run.epic)?;
        let outside: Vec<String> = input
            .nodes
            .iter()
            .map(|n| n.node.number)
            .filter(|n| !subs.contains(n))
            .map(|n| format!("#{n}"))
            .collect();
        if !outside.is_empty() {
            let known: Vec<String> = subs.iter().map(|n| format!("#{n}")).collect();
            return Err(LedgerError::Invalid(format!(
                "fora do épico #{}: {} (sub-issues do épico: {})",
                run.epic,
                outside.join(", "),
                if known.is_empty() {
                    "nenhuma no espelho".to_string()
                } else {
                    known.join(", ")
                }
            )));
        }
        let draft = EpicPlanDraft {
            nodes: input.nodes.iter().map(|n| n.node.clone()).collect(),
            summary: input.summary.clone(),
            proposals: input.proposals.clone(),
            session_id: Some(session_id.to_string()),
        };
        // Valida antes de mexer nas arestas.
        check_draft(run.epic, &draft)?;
        let tx = self.tx()?;
        for n in &subs {
            self.conn.execute(
                "DELETE FROM autopilot_dependency
                 WHERE project_id = ?1 AND number = ?2 AND source = 'analysis'",
                params![run.project_id, n],
            )?;
        }
        for n in &input.nodes {
            self.insert_analysis_edges(run.project_id, n.node.number, &n.blocked_by, now)?;
        }
        let plan = self.save_epic_plan_draft(run_id, &draft, now)?;
        tx.commit()?;
        Ok(plan)
    }

    // --- nós -------------------------------------------------------------

    /// Os nós da execução, por número.
    pub fn epic_nodes(&self, run_id: i64) -> Result<Vec<EpicNode>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {NODE_COLS} FROM epic_node WHERE run_id = ?1 ORDER BY number"
        ))?;
        let rows = stmt.query_map([run_id], row_to_node)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn epic_node(&self, run_id: i64, number: i64) -> Result<Option<EpicNode>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {NODE_COLS} FROM epic_node WHERE run_id = ?1 AND number = ?2"),
                params![run_id, number],
                row_to_node,
            )
            .optional()?)
    }

    fn require_epic_node(&self, run_id: i64, number: i64) -> Result<EpicNode> {
        self.epic_node(run_id, number)?.ok_or_else(|| {
            LedgerError::NotFound("nó da execução", format!("#{number} da execução {run_id}"))
        })
    }

    /// Muda o estado do nó, recusando a transição inválida
    /// ([`EpicNodeState::can_transition`]). Entrar em `assumida` ou
    /// `estacionada` guarda o estado de onde veio (o de antes do
    /// estacionamento, se vier de `estacionada`); `assumida` grava
    /// `assumed_since`. `reason` substitui o motivo do nó.
    pub fn set_epic_node_state(
        &self,
        run_id: i64,
        number: i64,
        to: EpicNodeState,
        reason: Option<&str>,
        now: i64,
    ) -> Result<EpicNode> {
        let tx = self.tx()?;
        let node = self.require_epic_node(run_id, number)?;
        if !node.state.can_transition(to) {
            return Err(LedgerError::Conflict(format!(
                "transição inválida do nó #{number}: {} -> {to}",
                node.state
            )));
        }
        let parked =
            |s: EpicNodeState| matches!(s, EpicNodeState::Assumida | EpicNodeState::Estacionada);
        let prev = if parked(to) {
            if parked(node.state) {
                node.prev_state
            } else {
                Some(node.state)
            }
        } else {
            None
        };
        let assumed_since = match (to, node.state) {
            (EpicNodeState::Assumida, EpicNodeState::Assumida) => node.assumed_since,
            (EpicNodeState::Assumida, _) => Some(now),
            _ => None,
        };
        self.conn.execute(
            "UPDATE epic_node SET state = ?3, prev_state = ?4, assumed_since = ?5, reason = ?6,
                    updated_at = ?7
             WHERE run_id = ?1 AND number = ?2",
            params![
                run_id,
                number,
                to.as_str(),
                prev.map(|p| p.as_str()),
                assumed_since,
                reason.map(|r| clip(r, SHORT_MAX_CHARS)),
                now
            ],
        )?;
        tx.commit()?;
        self.require_epic_node(run_id, number)
    }

    /// Volta o nó `assumida` ou `estacionada` ao estado de antes
    /// ("devolver ao motor", retomada depois do estacionamento). Sem estado
    /// anterior de sessão: `em_pr` se o nó tem PR, senão `em_execucao`.
    pub fn restore_epic_node(&self, run_id: i64, number: i64, now: i64) -> Result<EpicNode> {
        let node = self.require_epic_node(run_id, number)?;
        if !matches!(
            node.state,
            EpicNodeState::Assumida | EpicNodeState::Estacionada
        ) {
            return Err(LedgerError::Conflict(format!(
                "o nó #{number} não está assumido nem estacionado ({})",
                node.state
            )));
        }
        let to = match node.prev_state {
            Some(s @ (EpicNodeState::EmExecucao | EpicNodeState::EmPr)) => s,
            _ if node.pr.is_some() => EpicNodeState::EmPr,
            _ => EpicNodeState::EmExecucao,
        };
        self.set_epic_node_state(run_id, number, to, None, now)
    }

    /// Muda os campos do nó além do estado ([`EpicNodePatch`]).
    pub fn update_epic_node(
        &self,
        run_id: i64,
        number: i64,
        p: &EpicNodePatch,
        now: i64,
    ) -> Result<EpicNode> {
        let tx = self.tx()?;
        let mut n = self.require_epic_node(run_id, number)?;
        if let Some(v) = &p.session_id {
            n.session_id = v.clone();
        }
        if let Some(v) = &p.worktree {
            n.worktree = v.clone();
        }
        if let Some(v) = p.pr {
            n.pr = v;
        }
        if let Some(v) = p.attempts {
            n.attempts = v;
        }
        if let Some(v) = &p.reason {
            n.reason = v.as_deref().map(|r| clip(r, SHORT_MAX_CHARS));
        }
        if let Some(v) = &p.rebase_head {
            n.rebase_head = v.clone();
        }
        if let Some(v) = p.rebase_at {
            n.rebase_at = v;
        }
        self.conn.execute(
            "UPDATE epic_node SET session_id = ?3, worktree = ?4, pr = ?5, attempts = ?6,
                    reason = ?7, rebase_head = ?8, rebase_at = ?9, updated_at = ?10
             WHERE run_id = ?1 AND number = ?2",
            params![
                run_id,
                number,
                n.session_id,
                n.worktree,
                n.pr,
                n.attempts,
                n.reason,
                n.rebase_head,
                n.rebase_at,
                now
            ],
        )?;
        tx.commit()?;
        self.require_epic_node(run_id, number)
    }

    /// A sessão de um nó como o motor a vê: estado e, ociosa, desde quando
    /// (o último `session_state` para `idle`, como no piloto); `None` = o
    /// hub ainda não a conhece.
    pub fn epic_session_info(&self, id: &str) -> Result<Option<EpicSessionInfo>> {
        Ok(self
            .conn
            .query_row(
                "SELECT s.state,
                        coalesce((SELECT max(e.ts) FROM event e
                                  WHERE e.work_item_id = s.work_item_id AND e.session_id = s.id
                                    AND e.type = 'session_state'
                                    AND json_extract(e.payload, '$.to') = 'idle'),
                                 s.last_event_at),
                        s.cwd
                 FROM session s WHERE s.id = ?1",
                [id],
                |r| {
                    Ok(EpicSessionInfo {
                        state: enum_col(r, 0)?,
                        idle_since: r.get(1)?,
                        cwd: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// Última entrada humana (evento `human_input` do ambiente, #301) na
    /// sessão.
    pub fn last_human_input(&self, session_id: &str) -> Result<Option<i64>> {
        Ok(self.conn.query_row(
            "SELECT max(ts) FROM event WHERE session_id = ?1 AND type = 'human_input'",
            [session_id],
            |r| r.get(0),
        )?)
    }

    /// O último evento `kind` do nó na execução (ms).
    pub fn epic_node_event_at(&self, run_id: i64, number: i64, kind: &str) -> Result<Option<i64>> {
        Ok(self.conn.query_row(
            "SELECT max(at) FROM epic_run_event WHERE run_id = ?1 AND number = ?2 AND kind = ?3",
            params![run_id, number, kind],
            |r| r.get(0),
        )?)
    }

    // --- pedidos de decisão ---------------------------------------------

    /// Abre um pedido de decisão, com as opções da origem
    /// ([`EpicDecisionOrigin::options`]). Já há um pendente da mesma origem
    /// para o mesmo nó = devolve esse, sem duplicar.
    pub fn add_epic_decision(
        &self,
        run_id: i64,
        number: Option<i64>,
        origin: EpicDecisionOrigin,
        text: &str,
        now: i64,
    ) -> Result<EpicDecision> {
        let tx = self.tx()?;
        self.require_epic_run(run_id)?;
        let pending: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM epic_decision
                 WHERE run_id = ?1 AND number IS ?2 AND origin = ?3 AND answer IS NULL",
                params![run_id, number, origin.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        let id = match pending {
            Some(id) => id,
            None => {
                self.conn.execute(
                    "INSERT INTO epic_decision (run_id, number, origin, text, options,
                                                created_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        run_id,
                        number,
                        origin.as_str(),
                        clip(text, LONG_MAX_CHARS),
                        serde_json::to_string(origin.options()).unwrap_or_else(|_| "[]".into()),
                        now
                    ],
                )?;
                self.conn.last_insert_rowid()
            }
        };
        tx.commit()?;
        self.require_epic_decision(id)
    }

    pub fn epic_decision(&self, id: i64) -> Result<Option<EpicDecision>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {DECISION_COLS} FROM epic_decision WHERE id = ?1"),
                [id],
                row_to_decision,
            )
            .optional()?)
    }

    fn require_epic_decision(&self, id: i64) -> Result<EpicDecision> {
        self.epic_decision(id)?
            .ok_or_else(|| LedgerError::NotFound("pedido de decisão", id.to_string()))
    }

    /// Os pedidos da execução, do mais antigo para o mais novo; com
    /// `pending_only`, só os sem resposta.
    pub fn epic_decisions(&self, run_id: i64, pending_only: bool) -> Result<Vec<EpicDecision>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {DECISION_COLS} FROM epic_decision
             WHERE run_id = ?1 AND (NOT ?2 OR answer IS NULL) ORDER BY id"
        ))?;
        let rows = stmt.query_map(params![run_id, pending_only], row_to_decision)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Responde o pedido com uma das opções dele. Já respondido =
    /// `Conflict`.
    pub fn answer_epic_decision(
        &self,
        id: i64,
        answer: &str,
        by: &str,
        now: i64,
    ) -> Result<EpicDecision> {
        let tx = self.tx()?;
        let d = self.require_epic_decision(id)?;
        if d.answer.is_some() {
            return Err(LedgerError::Conflict(format!(
                "o pedido {id} já foi respondido"
            )));
        }
        if !d.options.iter().any(|o| o == answer) {
            return Err(LedgerError::Invalid(format!(
                "resposta inválida: {answer} (aceitas: {})",
                d.options.join(", ")
            )));
        }
        self.conn.execute(
            "UPDATE epic_decision SET answer = ?2, answered_at = ?3, answered_by = ?4
             WHERE id = ?1",
            params![id, answer, now, by],
        )?;
        tx.commit()?;
        self.require_epic_decision(id)
    }

    // --- eventos ---------------------------------------------------------

    /// Grava um evento do motor; devolve o id.
    pub fn add_epic_run_event(&self, e: &EpicRunEvent) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO epic_run_event (run_id, at, kind, number, reason, message)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![e.run_id, e.at, e.kind, e.number, e.reason, e.message],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Os últimos `n` eventos da execução, do mais antigo ao mais novo.
    pub fn epic_run_events_tail(&self, run_id: i64, n: usize) -> Result<Vec<EpicRunEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, run_id, at, kind, number, reason, message FROM epic_run_event
             WHERE run_id = ?1 ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![run_id, n as i64], |r| {
            Ok(EpicRunEvent {
                id: r.get(0)?,
                run_id: r.get(1)?,
                at: r.get(2)?,
                kind: r.get(3)?,
                number: r.get(4)?,
                reason: r.get(5)?,
                message: r.get(6)?,
            })
        })?;
        let mut v: Vec<EpicRunEvent> = rows.collect::<rusqlite::Result<_>>()?;
        v.reverse();
        Ok(v)
    }

    /// Eventos da execução depois do id `after` (0 = desde o início), em
    /// ordem, até `limit`.
    pub fn epic_run_events(
        &self,
        run_id: i64,
        after: i64,
        limit: usize,
    ) -> Result<Vec<EpicRunEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, run_id, at, kind, number, reason, message FROM epic_run_event
             WHERE run_id = ?1 AND id > ?2 ORDER BY id LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![run_id, after, limit as i64], |r| {
            Ok(EpicRunEvent {
                id: r.get(0)?,
                run_id: r.get(1)?,
                at: r.get(2)?,
                kind: r.get(3)?,
                number: r.get(4)?,
                reason: r.get(5)?,
                message: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ManualEdit, NewHubProject};

    fn hub() -> (Hub, i64) {
        let hub = Hub::open_in_memory().unwrap();
        let p = hub
            .add_project(&NewHubProject {
                owner: "acme".into(),
                repo: "acme-web".into(),
                name: "acme-web".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        (hub, p.project.id)
    }

    fn new_run(pid: i64, epic: i64, env: &str) -> NewEpicRun {
        NewEpicRun {
            project_id: pid,
            epic,
            env_id: env.into(),
            started_by: "fulano".into(),
            max_parallel: None,
            plan_model: None,
            plan_effort: None,
        }
    }

    fn node(n: i64, model: &str, areas: &[&str]) -> EpicPlanNodeInput {
        EpicPlanNodeInput {
            number: n,
            model: Some(model.into()),
            effort: None,
            areas: areas.iter().map(|a| a.to_string()).collect(),
            reason: format!("motivo da #{n}"),
        }
    }

    fn draft(nodes: Vec<EpicPlanNodeInput>) -> EpicPlanDraft {
        EpicPlanDraft {
            nodes,
            summary: "plano".into(),
            proposals: vec![EpicProposal {
                kind: "nova".into(),
                title: "Faltou a migração".into(),
                body: String::new(),
                from: None,
            }],
            session_id: Some("s-plano".into()),
        }
    }

    fn approval(version: i64) -> EpicApproval {
        EpicApproval {
            version,
            approved_by: "fulano".into(),
            max_parallel: 3,
            code_review: true,
            fingerprints: [(11, "f11".to_string())].into(),
        }
    }

    /// Execução já aprovada com as sub-issues 11 e 12 (12 bloqueada pela 11).
    fn running(hub: &Hub, pid: i64) -> EpicRun {
        let run = hub.create_epic_run(&new_run(pid, 10, "env-a"), 1).unwrap();
        hub.set_dependency_manual(pid, 12, 11, ManualEdit::Add, "precisa da 11", 1)
            .unwrap();
        hub.save_epic_plan_draft(
            run.id,
            &draft(vec![
                node(11, "opus", &["crates/a/"]),
                node(12, "haiku", &[]),
            ]),
            2,
        )
        .unwrap();
        hub.approve_epic_plan(run.id, &approval(1), 3).unwrap()
    }

    #[test]
    fn cria_execucao_em_planejando_uma_ativa_por_epico() {
        let (hub, pid) = hub();
        let r = hub.create_epic_run(&new_run(pid, 10, "env-a"), 5).unwrap();
        assert_eq!(r.status, EpicRunStatus::Planejando);
        assert_eq!((r.owner.as_str(), r.epic, r.max_parallel), ("acme", 10, 2));
        assert_eq!((r.plan_version, r.plan_session.as_deref()), (None, None));
        let r = hub
            .set_epic_run_plan_session(r.id, Some("s-plano"), 5)
            .unwrap();
        assert_eq!(r.plan_session.as_deref(), Some("s-plano"));
        let r = hub
            .set_epic_run_integration_session(r.id, Some("s-int"), 6)
            .unwrap();
        assert_eq!(r.integration_session.as_deref(), Some("s-int"));
        // Outra para o mesmo épico: recusada.
        assert!(matches!(
            hub.create_epic_run(&new_run(pid, 10, "env-b"), 6),
            Err(LedgerError::Conflict(_))
        ));
        // Outro épico no mesmo ambiente: aceita.
        hub.create_epic_run(&new_run(pid, 20, "env-a"), 6).unwrap();
        assert_eq!(hub.active_epic_runs().unwrap().len(), 2);
        // Cancelada, o épico aceita outra.
        hub.set_epic_run_status(r.id, EpicRunStatus::Cancelado, None, None, 7)
            .unwrap();
        let again = hub.create_epic_run(&new_run(pid, 10, "env-a"), 8).unwrap();
        assert_eq!(hub.active_epic_run(pid, 10).unwrap().unwrap().id, again.id);
        assert_eq!(hub.epic_runs(pid, 10).unwrap().len(), 2);
        let cancelled = hub.epic_run(r.id).unwrap().unwrap();
        assert_eq!(cancelled.ended_at, Some(7));
    }

    #[test]
    fn criacao_recusa_dados_invalidos() {
        let (hub, pid) = hub();
        let mut n = new_run(pid, 10, "env-a");
        n.max_parallel = Some(0);
        assert!(hub.create_epic_run(&n, 1).is_err());
        n.max_parallel = Some(EPIC_RUN_PARALLEL_CEILING + 1);
        assert!(hub.create_epic_run(&n, 1).is_err());
        assert!(hub.create_epic_run(&new_run(pid, 0, "env-a"), 1).is_err());
        assert!(hub.create_epic_run(&new_run(pid, 10, " "), 1).is_err());
        assert!(matches!(
            hub.create_epic_run(&new_run(pid + 99, 10, "env-a"), 1),
            Err(LedgerError::NotFound(..))
        ));
    }

    #[test]
    fn rascunho_vai_para_aguardando_aprovacao_com_todos_os_campos() {
        let (hub, pid) = hub();
        let run = hub.create_epic_run(&new_run(pid, 10, "env-a"), 1).unwrap();
        hub.set_dependency_manual(pid, 12, 11, ManualEdit::Add, "precisa da 11", 1)
            .unwrap();
        // Aresta de outra issue fora do plano não entra.
        hub.set_dependency_manual(pid, 99, 11, ManualEdit::Add, "", 1)
            .unwrap();
        let mut d = draft(vec![
            node(12, "haiku", &["./ui/src/", "ui/src/"]),
            node(11, "opus", &["crates/a/**"]),
        ]);
        d.nodes[1].effort = Some("high".into());
        let plan = hub.save_epic_plan_draft(run.id, &d, 2).unwrap();
        assert_eq!((plan.version, plan.state), (1, EpicPlanState::Rascunho));
        assert_eq!(plan.session_id.as_deref(), Some("s-plano"));
        assert_eq!(plan.proposals.len(), 1);
        let nodes: Vec<_> = plan
            .nodes
            .iter()
            .map(|n| (n.number, n.position, n.areas.clone()))
            .collect();
        assert_eq!(
            nodes,
            vec![
                (12, 0, vec!["ui/src/".to_string()]),
                (11, 1, vec!["crates/a/**".to_string()])
            ]
        );
        assert_eq!(plan.nodes[1].effort.as_deref(), Some("high"));
        assert_eq!(
            plan.edges
                .iter()
                .map(|e| (e.number, e.blocker))
                .collect::<Vec<_>>(),
            vec![(12, 11)]
        );
        let r = hub.epic_run(run.id).unwrap().unwrap();
        assert_eq!(r.status, EpicRunStatus::AguardandoAprovacao);
        // Regravar antes de aprovar substitui o rascunho (mesma versão).
        let again = hub
            .save_epic_plan_draft(run.id, &draft(vec![node(11, "opus", &[])]), 3)
            .unwrap();
        assert_eq!((again.version, again.nodes.len()), (1, 1));
    }

    #[test]
    fn rascunho_invalido_e_recusado() {
        let (hub, pid) = hub();
        let run = hub.create_epic_run(&new_run(pid, 10, "env-a"), 1).unwrap();
        let bad = [
            draft(vec![]),
            draft(vec![node(11, "opus", &[]), node(11, "opus", &[])]),
            draft(vec![node(10, "opus", &[])]),
            draft(vec![node(-1, "opus", &[])]),
            draft(vec![node(11, "gpt", &[])]),
            draft(vec![node(11, "opus", &["/etc"])]),
            {
                let mut d = draft(vec![node(11, "opus", &[])]);
                d.proposals[0].kind = "dividir".into();
                d
            },
        ];
        for d in &bad {
            assert!(
                matches!(
                    hub.save_epic_plan_draft(run.id, d, 2),
                    Err(LedgerError::Invalid(_))
                ),
                "{d:?}"
            );
        }
        assert_eq!(hub.latest_epic_plan(run.id).unwrap(), None);
    }

    #[test]
    fn aprovacao_congela_versao_e_cria_os_nos() {
        let (hub, pid) = hub();
        let run = running(&hub, pid);
        assert_eq!(run.status, EpicRunStatus::Executando);
        assert_eq!(
            (run.plan_version, run.max_parallel, run.code_review),
            (Some(1), 3, true)
        );
        let nodes: Vec<_> = hub
            .epic_nodes(run.id)
            .unwrap()
            .into_iter()
            .map(|n| (n.number, n.state))
            .collect();
        assert_eq!(
            nodes,
            vec![(11, EpicNodeState::Pendente), (12, EpicNodeState::Pendente)]
        );
        // Mudar a aresta depois não muda a versão aprovada.
        hub.set_dependency_manual(pid, 12, 11, ManualEdit::Remove, "", 9)
            .unwrap();
        let plan = hub.epic_plan(run.id, 1).unwrap().unwrap();
        assert_eq!(plan.state, EpicPlanState::Aprovado);
        assert_eq!(plan.approved_by.as_deref(), Some("fulano"));
        assert_eq!(plan.edges.len(), 1);
        assert_eq!(plan.edges[0].source, "manual");
        // Impressão digital congelada (a 12 não tinha).
        assert_eq!(plan.nodes[0].fingerprint.as_deref(), Some("f11"));
        assert_eq!(plan.nodes[1].fingerprint, None);
        hub.set_epic_plan_fingerprint(run.id, 12, "f12").unwrap();
        assert!(hub.set_epic_plan_fingerprint(run.id, 99, "x").is_err());
        let plan = hub.epic_plan(run.id, 1).unwrap().unwrap();
        assert_eq!(plan.nodes[1].fingerprint.as_deref(), Some("f12"));
    }

    #[test]
    fn aprovacao_recusa_versao_velha_e_no_sem_modelo() {
        let (hub, pid) = hub();
        let run = hub.create_epic_run(&new_run(pid, 10, "env-a"), 1).unwrap();
        // Sem plano.
        assert!(hub.approve_epic_plan(run.id, &approval(1), 2).is_err());
        let mut d = draft(vec![node(11, "opus", &[])]);
        d.nodes[0].model = None;
        hub.save_epic_plan_draft(run.id, &d, 2).unwrap();
        assert!(matches!(
            hub.approve_epic_plan(run.id, &approval(1), 3),
            Err(LedgerError::Conflict(_))
        ));
        hub.save_epic_plan_draft(run.id, &draft(vec![node(11, "opus", &[])]), 4)
            .unwrap();
        assert!(hub.approve_epic_plan(run.id, &approval(2), 5).is_err());
        let mut a = approval(1);
        a.max_parallel = 0;
        assert!(hub.approve_epic_plan(run.id, &a, 5).is_err());
        hub.approve_epic_plan(run.id, &approval(1), 5).unwrap();
        // Aprovada, não aprova de novo.
        assert!(hub.approve_epic_plan(run.id, &approval(1), 6).is_err());
    }

    #[test]
    fn replanejamento_cria_versao_nova_e_preserva_nos_mesclados() {
        let (hub, pid) = hub();
        let run = running(&hub, pid);
        for to in [
            EpicNodeState::Pronta,
            EpicNodeState::EmExecucao,
            EpicNodeState::EmPr,
            EpicNodeState::Mesclada,
        ] {
            hub.set_epic_node_state(run.id, 11, to, None, 4).unwrap();
        }
        hub.set_epic_run_status(run.id, EpicRunStatus::Replanejando, None, None, 5)
            .unwrap();
        // v2: a 12 sai do plano (não começou), entra a 13.
        let plan = hub
            .save_epic_plan_draft(
                run.id,
                &draft(vec![node(11, "opus", &[]), node(13, "sonnet", &[])]),
                6,
            )
            .unwrap();
        assert_eq!(plan.version, 2);
        let r = hub.approve_epic_plan(run.id, &approval(2), 7).unwrap();
        assert_eq!(r.plan_version, Some(2));
        let nodes: Vec<_> = hub
            .epic_nodes(run.id)
            .unwrap()
            .into_iter()
            .map(|n| (n.number, n.state))
            .collect();
        assert_eq!(
            nodes,
            vec![(11, EpicNodeState::Mesclada), (13, EpicNodeState::Pendente)]
        );
        assert_eq!(
            hub.epic_plan(run.id, 1).unwrap().unwrap().state,
            EpicPlanState::Substituido
        );
        assert_eq!(
            hub.latest_epic_plan(run.id).unwrap().unwrap().state,
            EpicPlanState::Aprovado
        );
    }

    #[test]
    fn transicoes_invalidas_da_execucao_sao_recusadas() {
        let (hub, pid) = hub();
        let run = hub.create_epic_run(&new_run(pid, 10, "env-a"), 1).unwrap();
        for to in [
            EpicRunStatus::Executando,
            EpicRunStatus::Integrando,
            EpicRunStatus::Pausado,
            EpicRunStatus::Concluido,
        ] {
            assert!(matches!(
                hub.set_epic_run_status(run.id, to, None, None, 2),
                Err(LedgerError::Conflict(_))
            ));
        }
        // Aprovar não vale sem passar por aguardando_aprovacao.
        assert!(hub.approve_epic_plan(run.id, &approval(1), 2).is_err());
        assert_eq!(
            hub.epic_run(run.id).unwrap().unwrap().status,
            EpicRunStatus::Planejando
        );
    }

    #[test]
    fn pausa_e_estacionamento_guardam_motivo_e_retomada() {
        let (hub, pid) = hub();
        let run = running(&hub, pid);
        // Estacionar sem resume_at: recusado.
        assert!(hub
            .set_epic_run_status(run.id, EpicRunStatus::Estacionado, None, None, 4)
            .is_err());
        let r = hub
            .set_epic_run_status(
                run.id,
                EpicRunStatus::Estacionado,
                Some("five_hour"),
                Some(1_000),
                4,
            )
            .unwrap();
        assert_eq!(
            (r.status, r.paused_reason.as_deref(), r.resume_at),
            (EpicRunStatus::Estacionado, Some("five_hour"), Some(1_000))
        );
        let r = hub.set_epic_run_resume_at(run.id, 2_000, 4).unwrap();
        assert_eq!(r.resume_at, Some(2_000));
        // Pausa manual sobre o estacionamento: some o resume_at.
        let r = hub
            .set_epic_run_status(run.id, EpicRunStatus::Pausado, Some("almoço"), Some(9), 5)
            .unwrap();
        assert_eq!(
            (r.paused_reason.as_deref(), r.resume_at),
            (Some("almoço"), None)
        );
        // Só a estacionada reagenda.
        assert!(hub.set_epic_run_resume_at(run.id, 2_000, 5).is_err());
        // Pausado não estaciona.
        assert!(hub
            .set_epic_run_status(run.id, EpicRunStatus::Estacionado, None, Some(9), 6)
            .is_err());
        let r = hub
            .set_epic_run_status(run.id, EpicRunStatus::Executando, None, None, 6)
            .unwrap();
        assert_eq!(
            (r.paused_reason, r.resume_at, r.ended_at),
            (None, None, None)
        );
        let r = hub.set_epic_run_parallel(run.id, 5, 7).unwrap();
        assert_eq!(r.max_parallel, 5);
        assert!(hub.set_epic_run_parallel(run.id, 9, 7).is_err());
        hub.set_epic_run_status(run.id, EpicRunStatus::Cancelado, None, None, 8)
            .unwrap();
        assert!(hub.set_epic_run_parallel(run.id, 2, 9).is_err());
        assert!(hub
            .set_epic_run_status(run.id, EpicRunStatus::Executando, None, None, 9)
            .is_err());
    }

    #[test]
    fn transicoes_invalidas_do_no_sao_recusadas() {
        let (hub, pid) = hub();
        let run = running(&hub, pid);
        assert!(matches!(
            hub.set_epic_node_state(run.id, 11, EpicNodeState::EmPr, None, 4),
            Err(LedgerError::Conflict(_))
        ));
        assert!(matches!(
            hub.set_epic_node_state(run.id, 99, EpicNodeState::Pronta, None, 4),
            Err(LedgerError::NotFound(..))
        ));
        for to in [
            EpicNodeState::Pronta,
            EpicNodeState::EmExecucao,
            EpicNodeState::Mesclada,
        ] {
            hub.set_epic_node_state(run.id, 11, to, None, 4).unwrap();
        }
        assert!(hub
            .set_epic_node_state(run.id, 11, EpicNodeState::EmExecucao, None, 5)
            .is_err());
    }

    #[test]
    fn assumida_e_devolvida_volta_ao_estado_anterior() {
        let (hub, pid) = hub();
        let run = running(&hub, pid);
        for to in [
            EpicNodeState::Pronta,
            EpicNodeState::EmExecucao,
            EpicNodeState::EmPr,
        ] {
            hub.set_epic_node_state(run.id, 11, to, None, 4).unwrap();
        }
        hub.update_epic_node(
            run.id,
            11,
            &EpicNodePatch {
                session_id: Some(Some("s-11".into())),
                worktree: Some(Some("wt/11".into())),
                pr: Some(Some(31)),
                attempts: Some(2),
                rebase_head: Some(Some("abc".into())),
                ..Default::default()
            },
            4,
        )
        .unwrap();
        // Estacionada e depois assumida: guarda o em_pr de antes.
        hub.set_epic_node_state(run.id, 11, EpicNodeState::Estacionada, None, 5)
            .unwrap();
        let n = hub
            .set_epic_node_state(run.id, 11, EpicNodeState::Assumida, None, 6)
            .unwrap();
        assert_eq!(
            (n.state, n.prev_state, n.assumed_since),
            (EpicNodeState::Assumida, Some(EpicNodeState::EmPr), Some(6))
        );
        assert_eq!(
            (n.session_id.as_deref(), n.pr, n.attempts),
            (Some("s-11"), Some(31), 2)
        );
        assert_eq!((n.rebase_head.as_deref(), n.rebase_at), (Some("abc"), None));
        let n = hub.restore_epic_node(run.id, 11, 7).unwrap();
        assert_eq!(
            (n.state, n.prev_state, n.assumed_since),
            (EpicNodeState::EmPr, None, None)
        );
        assert!(hub.restore_epic_node(run.id, 11, 8).is_err());
        // Limpar campos.
        let n = hub
            .update_epic_node(
                run.id,
                11,
                &EpicNodePatch {
                    worktree: Some(None),
                    reason: Some(Some("mudança não commitada".into())),
                    ..Default::default()
                },
                9,
            )
            .unwrap();
        assert_eq!(
            (n.worktree, n.reason.as_deref()),
            (None, Some("mudança não commitada"))
        );
    }

    #[test]
    fn pedidos_de_decisao_sem_duplicar_e_com_resposta_valida() {
        let (hub, pid) = hub();
        let run = running(&hub, pid);
        let d = hub
            .add_epic_decision(
                run.id,
                Some(11),
                EpicDecisionOrigin::CiEsgotado,
                "CI falhou 3x",
                4,
            )
            .unwrap();
        assert_eq!(d.options, vec!["assumir", "mais_tentativas", "replanejar"]);
        let same = hub
            .add_epic_decision(
                run.id,
                Some(11),
                EpicDecisionOrigin::CiEsgotado,
                "de novo",
                5,
            )
            .unwrap();
        assert_eq!(same.id, d.id);
        hub.add_epic_decision(run.id, None, EpicDecisionOrigin::IssueAlterada, "mudou", 5)
            .unwrap();
        assert_eq!(hub.epic_decisions(run.id, true).unwrap().len(), 2);
        assert!(matches!(
            hub.answer_epic_decision(d.id, "talvez", "fulano", 6),
            Err(LedgerError::Invalid(_))
        ));
        let a = hub
            .answer_epic_decision(d.id, "mais_tentativas", "fulano", 6)
            .unwrap();
        assert_eq!(
            (a.answer.as_deref(), a.answered_at, a.answered_by.as_deref()),
            (Some("mais_tentativas"), Some(6), Some("fulano"))
        );
        assert!(hub
            .answer_epic_decision(d.id, "assumir", "fulano", 7)
            .is_err());
        assert_eq!(hub.epic_decisions(run.id, true).unwrap().len(), 1);
        assert_eq!(hub.epic_decisions(run.id, false).unwrap().len(), 2);
        // Respondido, a mesma situação abre um pedido novo.
        let novo = hub
            .add_epic_decision(run.id, Some(11), EpicDecisionOrigin::CiEsgotado, "", 8)
            .unwrap();
        assert_ne!(novo.id, d.id);
    }

    #[test]
    fn eventos_em_ordem_depois_do_id() {
        let (hub, pid) = hub();
        let run = running(&hub, pid);
        for (i, k) in ["started", "merged", "worktree_removed"].iter().enumerate() {
            hub.add_epic_run_event(&EpicRunEvent {
                run_id: run.id,
                at: i as i64,
                kind: k.to_string(),
                number: Some(11),
                message: format!("evento {k}"),
                ..Default::default()
            })
            .unwrap();
        }
        let all = hub.epic_run_events(run.id, 0, 100).unwrap();
        assert_eq!(all.len(), 3);
        let after = hub.epic_run_events(run.id, all[0].id, 1).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].kind, "merged");
    }

    /// Migração 46 num `hub.db` da 45: o projeto e as arestas do piloto
    /// ficam, e o épico do projeto aceita uma execução com plano.
    #[test]
    fn migracao_46_sobe_um_banco_da_45() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("hub.db");
        {
            let conn = rusqlite::Connection::open(&db).unwrap();
            for m in &crate::HUB_MIGRATIONS[..45] {
                conn.execute_batch(m).unwrap();
            }
            conn.execute_batch(
                "PRAGMA user_version = 45;
                 INSERT INTO project (key, gh_owner, gh_repo, name, created_at, rev)
                   VALUES ('acme/acme-web', 'acme', 'acme-web', 'acme-web', 1, 1);
                 INSERT INTO autopilot_dependency (project_id, number, blocker, reason, source,
                                                   updated_at)
                   SELECT id, 12, 11, 'antes', 'analysis', 1 FROM project;",
            )
            .unwrap();
        }
        let hub = Hub::open(&db).unwrap();
        let v: i64 = hub
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, crate::HUB_MIGRATIONS.len() as i64);
        let pid = hub.find_project("acme", "acme-web").unwrap().unwrap().id;
        let run = running(&hub, pid);
        let plan = hub.epic_plan(run.id, 1).unwrap().unwrap();
        // A manual de `running` prevalece sobre a análise que já existia.
        assert_eq!(plan.edges.len(), 1);
        assert_eq!((plan.edges[0].number, plan.edges[0].blocker), (12, 11));
    }

    #[test]
    fn estado_sobrevive_a_reabrir_o_store() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("hub.db");
        let (run_id, pid) = {
            let hub = Hub::open(&db).unwrap();
            let p = hub
                .add_project(&NewHubProject {
                    owner: "acme".into(),
                    repo: "acme-web".into(),
                    name: "acme-web".into(),
                    base_branch: "develop".into(),
                    protected_branches: vec![],
                })
                .unwrap();
            let run = running(&hub, p.project.id);
            hub.set_epic_node_state(run.id, 11, EpicNodeState::Pronta, None, 4)
                .unwrap();
            hub.add_epic_decision(run.id, Some(12), EpicDecisionOrigin::PlanoErrado, "x", 4)
                .unwrap();
            hub.set_epic_run_status(run.id, EpicRunStatus::Estacionado, None, Some(77), 5)
                .unwrap();
            (run.id, p.project.id)
        };
        let hub = Hub::open(&db).unwrap();
        let run = hub.active_epic_run(pid, 10).unwrap().unwrap();
        assert_eq!(run.id, run_id);
        assert_eq!(
            (run.status, run.resume_at, run.plan_version),
            (EpicRunStatus::Estacionado, Some(77), Some(1))
        );
        assert_eq!(
            hub.epic_node(run_id, 11).unwrap().unwrap().state,
            EpicNodeState::Pronta
        );
        assert_eq!(hub.epic_decisions(run_id, true).unwrap().len(), 1);
        assert_eq!(hub.epic_plan(run_id, 1).unwrap().unwrap().edges.len(), 1);
    }

    #[test]
    fn sub_issues_presas_a_execucao_ativa() {
        use crate::{GithubSnapshot, IssueRow};
        let (hub, pid) = hub();
        let issue = |n: i64, parent: Option<i64>| IssueRow {
            gh_number: n,
            title: format!("ACME-{n}"),
            state: "open".into(),
            url: String::new(),
            updated_at: 1,
            parent_number: parent,
            ..Default::default()
        };
        hub.apply_github(
            &GithubSnapshot {
                project_id: pid,
                issues: vec![issue(10, None), issue(13, Some(10)), issue(20, None)],
                ..Default::default()
            },
            "env-a",
        )
        .unwrap();
        // Planejando: só as filhas do espelho.
        let run = hub.create_epic_run(&new_run(pid, 10, "env-a"), 1).unwrap();
        assert_eq!(hub.epic_run_numbers(pid).unwrap(), [13].into());
        // Rascunho e depois nós.
        hub.save_epic_plan_draft(run.id, &draft(vec![node(11, "opus", &[])]), 2)
            .unwrap();
        assert_eq!(hub.epic_run_numbers(pid).unwrap(), [11, 13].into());
        hub.approve_epic_plan(run.id, &approval(1), 3).unwrap();
        assert_eq!(hub.epic_run_numbers(pid).unwrap(), [11, 13].into());
        // Cancelada: voltam a ser elegíveis.
        hub.set_epic_run_status(run.id, EpicRunStatus::Cancelado, None, None, 4)
            .unwrap();
        assert!(hub.epic_run_numbers(pid).unwrap().is_empty());
    }

    #[test]
    fn plano_da_sessao_substitui_as_arestas_de_analise_do_epico() {
        use crate::{GithubSnapshot, IssueRow};
        let (hub, pid) = hub();
        let issue = |n: i64, parent: Option<i64>| IssueRow {
            gh_number: n,
            title: format!("ACME-{n}"),
            state: "open".into(),
            url: String::new(),
            updated_at: 1,
            parent_number: parent,
            ..Default::default()
        };
        hub.apply_github(
            &GithubSnapshot {
                project_id: pid,
                issues: vec![
                    issue(10, None),
                    issue(11, Some(10)),
                    issue(12, Some(10)),
                    issue(13, Some(10)),
                    issue(20, None),
                ],
                ..Default::default()
            },
            "env-a",
        )
        .unwrap();
        // Arestas de antes: análise da 13 (sai), manual da 12 (fica),
        // análise fora do épico (fica).
        let blocker = |n: i64| BlockerInput {
            number: n,
            reason: format!("precisa da #{n}"),
        };
        hub.insert_analysis_edges(pid, 13, &[blocker(11)], 1)
            .unwrap();
        hub.insert_analysis_edges(pid, 20, &[blocker(11)], 1)
            .unwrap();
        hub.set_dependency_manual(pid, 12, 99, ManualEdit::Add, "manual", 1)
            .unwrap();
        let run = hub.create_epic_run(&new_run(pid, 10, "env-a"), 1).unwrap();
        let sn = |n: i64, bs: &[i64]| EpicPlanSessionNode {
            node: node(n, "sonnet", &["crates/x/"]),
            blocked_by: bs.iter().map(|b| blocker(*b)).collect(),
        };
        let input = EpicPlanSessionInput {
            nodes: vec![sn(11, &[]), sn(12, &[11]), sn(13, &[])],
            summary: "resumo".into(),
            proposals: vec![],
        };
        // Nó fora do épico: recusado sem mexer em nada.
        let mut bad = input.clone();
        bad.nodes.push(sn(20, &[]));
        let e = hub
            .save_epic_plan_from_session(run.id, &bad, "s-plano", 2)
            .unwrap_err();
        assert!(e.to_string().contains("fora do épico #10: #20"), "{e}");
        assert_eq!(hub.latest_epic_plan(run.id).unwrap(), None);
        let plan = hub
            .save_epic_plan_from_session(run.id, &input, "s-plano", 3)
            .unwrap();
        assert_eq!(plan.session_id.as_deref(), Some("s-plano"));
        assert_eq!(plan.summary, "resumo");
        let mut edges: Vec<(i64, i64, String)> = hub
            .autopilot_dependencies(pid)
            .unwrap()
            .into_iter()
            .map(|e| (e.number, e.blocker, e.source))
            .collect();
        edges.sort();
        assert_eq!(
            edges,
            vec![
                (12, 11, "analysis".into()),
                (12, 99, "manual".into()),
                (20, 11, "analysis".into()),
            ]
        );
        assert_eq!(
            hub.epic_run(run.id).unwrap().unwrap().status,
            EpicRunStatus::AguardandoAprovacao
        );
        // O rascunho mostra as arestas vivas dos nós.
        assert_eq!(plan.edges.len(), 2);
    }

    #[test]
    fn excluir_o_projeto_apaga_as_execucoes() {
        let (hub, pid) = hub();
        let run = running(&hub, pid);
        hub.add_epic_decision(run.id, Some(11), EpicDecisionOrigin::SemProgresso, "", 4)
            .unwrap();
        hub.delete_project("acme", "acme-web").unwrap();
        assert_eq!(hub.epic_run(run.id).unwrap(), None);
        assert!(hub.epic_nodes(run.id).unwrap().is_empty());
        assert!(hub.epic_decisions(run.id, false).unwrap().is_empty());
        assert_eq!(hub.epic_plan(run.id, 1).unwrap(), None);
    }
}
