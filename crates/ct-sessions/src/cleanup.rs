//! Limpeza do ambiente depois da conclusão do work item (épico #284, issue
//! #287): a base comum às etapas de sessões (#285), worktree e `target/`
//! (#265) e branches (#286).
//!
//! - [`completion`]: o sinal único de conclusão. Lê só o ledger e o espelho
//!   da forja (`issue_cache`/`pr_cache`, mantido pelo sync periódico do
//!   GitHub com ETag, ou pelo do Jira e GitLab com o glab), então montar o
//!   plano não faz nenhuma chamada à API. O MR do GitLab mesclado ou fechado
//!   dá o mesmo sinal do PR.
//! - [`CleanupStep`]: o trait de cada etapa. Avalia (`plan`) sem mudar nada
//!   e executa (`run`) só o que avaliou como pendente.
//! - [`Cleaner`]: monta o [`CleanupPlan`] e executa o plano inteiro, um work
//!   item ou uma etapa ([`CleanupScope`]), com `dry_run`.
//!
//! Ordem por work item: sessões → worktree → `target/` → branch local →
//! branch remota ([`StepKind::ORDER`]). Uma etapa só roda se as anteriores
//! terminaram (`feita`); uma bloqueada segura as seguintes. Conservador:
//! etapa que falha ou fica bloqueada para o work item ali, e as idades
//! mínimas ([`CleanupConfig`]) bloqueiam tudo o que é recente demais.
//!
//! Exceção única: o bloqueio do worktree não segura a etapa `target/`
//! ([`independent`]). Apagar só o `target/` de um worktree que fica não
//! perde trabalho, e é justamente o caso do worktree que não pode sair
//! (#265). A branch local continua esperando o worktree.
//!
//! Além dos concluídos, o plano traz os work items em andamento cujo
//! worktree está inativo há `cleanup.target_idle_days`, só com a etapa
//! `target/` e `completion: None`
//! ([`crate::cleanup_worktree::inactive_targets`]).

pub use crate::cleanup_branches::{never_touch, BranchLocalStep, BranchRemoteStep, RemoteBranches};
pub use crate::cleanup_sessions::{
    agent_transcript_dirs, claude_project_slug, remove_agent_transcripts, AgentTranscripts,
    SessionsStep,
};
pub use crate::cleanup_worktree::{worktree_activity, TargetStep, WorktreeStep};
use crate::config::CleanupConfig;
use crate::{GithubSync, Result, Sessions, SessionsError, PROJECT_WORK_ITEM};
use ct_core::{PrState, WorkItemStatus};
use ct_ledger::{Ledger, Project, WorkItem};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

/// Bytes para pessoas, em base 1024 com uma casa decimal: `512 B`,
/// `1,5 KB`, `6,6 GB`.
pub fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if b < 1024 {
        return format!("{b} B");
    }
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u]).replace('.', ",")
}

/// Etapa da limpeza de um work item, na ordem em que roda.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum StepKind {
    /// Sessões vivas, interrompidas e transcripts do agente (#285).
    Sessoes,
    /// O worktree do work item (#265).
    Worktree,
    /// O `target/` de um worktree que fica (#265): o de concluído cujo
    /// worktree não pode sair e o de work item em andamento inativo.
    Target,
    /// A branch no clone principal (#286).
    BranchLocal,
    /// A branch em `origin` (#286).
    BranchRemota,
}

impl StepKind {
    pub const ORDER: [StepKind; 5] = [
        StepKind::Sessoes,
        StepKind::Worktree,
        StepKind::Target,
        StepKind::BranchLocal,
        StepKind::BranchRemota,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            StepKind::Sessoes => "sessoes",
            StepKind::Worktree => "worktree",
            StepKind::Target => "target",
            StepKind::BranchLocal => "branch_local",
            StepKind::BranchRemota => "branch_remota",
        }
    }

    /// Nome para pessoas (CLI, log).
    pub fn label(self) -> &'static str {
        match self {
            StepKind::Sessoes => "sessões",
            StepKind::Worktree => "worktree",
            StepKind::Target => "target/",
            StepKind::BranchLocal => "branch local",
            StepKind::BranchRemota => "branch remota",
        }
    }

    /// Sub-issue do épico #284 que implementa a etapa.
    pub fn issue(self) -> u32 {
        match self {
            StepKind::Sessoes => 285,
            StepKind::Worktree | StepKind::Target => 265,
            StepKind::BranchLocal | StepKind::BranchRemota => 286,
        }
    }
}

impl fmt::Display for StepKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for StepKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        StepKind::ORDER
            .into_iter()
            .find(|k| k.as_str() == s)
            .ok_or_else(|| {
                format!(
                    "etapa inválida: {s} (aceitas: {})",
                    StepKind::ORDER.map(StepKind::as_str).join(", ")
                )
            })
    }
}

/// Estado de uma etapa no plano.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum StepState {
    /// Há o que limpar e nada impede: roda no `cleanup.run` (e no job, com
    /// `cleanup.auto`).
    Pendente,
    /// Há o que limpar, mas algo impede (`reason`): nunca roda.
    Bloqueada,
    /// Nada a limpar nesta etapa.
    Feita,
}

/// Avaliação de uma etapa para um work item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct StepPlan {
    pub kind: StepKind,
    pub state: StepState,
    /// Por que está bloqueada (só em `bloqueada`).
    pub reason: Option<String>,
    /// Espaço estimado que a etapa libera, em bytes.
    pub bytes: u64,
    /// O que sairia (caminhos, sessões, branches), para o dry-run.
    pub targets: Vec<String>,
}

impl StepPlan {
    pub fn pendente(kind: StepKind, bytes: u64, targets: Vec<String>) -> Self {
        StepPlan {
            kind,
            state: StepState::Pendente,
            reason: None,
            bytes,
            targets,
        }
    }

    pub fn bloqueada(kind: StepKind, reason: impl Into<String>) -> Self {
        StepPlan {
            kind,
            state: StepState::Bloqueada,
            reason: Some(reason.into()),
            bytes: 0,
            targets: Vec::new(),
        }
    }

    pub fn feita(kind: StepKind) -> Self {
        StepPlan {
            kind,
            state: StepState::Feita,
            reason: None,
            bytes: 0,
            targets: Vec::new(),
        }
    }

    /// Bloqueia uma etapa pendente, mantendo o que ela sairia (o motivo diz
    /// o que falta).
    pub(crate) fn block(mut self, reason: impl Into<String>) -> Self {
        if self.state == StepState::Pendente {
            self.state = StepState::Bloqueada;
            self.reason = Some(reason.into());
        }
        self
    }
}

/// O que marcou o work item como concluído.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum CompletionSignal {
    /// Status `concluido` no ledger.
    WorkItemConcluido,
    /// PR (MR, no GitLab) da branch mesclado.
    PrMesclado,
    /// PR (MR, no GitLab) da branch fechado sem merge.
    PrFechado,
    /// Issue fechada no GitHub.
    IssueFechada,
}

impl CompletionSignal {
    pub fn label(self) -> &'static str {
        match self {
            CompletionSignal::WorkItemConcluido => "work item concluído",
            CompletionSignal::PrMesclado => "PR mesclado",
            CompletionSignal::PrFechado => "PR fechado",
            CompletionSignal::IssueFechada => "issue fechada",
        }
    }

    /// O rótulo com o nome da mudança na forja: `MR mesclado` no GitLab.
    pub fn label_for(self, forge: ct_core::ForgeKind) -> String {
        match self {
            CompletionSignal::PrMesclado | CompletionSignal::PrFechado => {
                self.label().replacen("PR", ct_core::change_label(forge), 1)
            }
            _ => self.label().to_string(),
        }
    }
}

/// Conclusão de um work item ([`completion`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Completion {
    pub signal: CompletionSignal,
    /// Desde quando (ms), se conhecido: base de `min_age_hours`.
    pub since: Option<i64>,
    /// PR que deu o sinal (`pr_mesclado`/`pr_fechado`).
    pub pr: Option<i64>,
}

/// Sinal único de conclusão do work item (épico #284): status `concluido`
/// no ledger ou no hub (`GET /sync/done`, #298), PR da branch mesclado ou
/// fechado, ou issue fechada. Um PR
/// aberto da branch vence os demais sinais (o trabalho continua). Lê só o
/// ledger e o espelho do GitHub, sem chamar a API: o sync periódico, com
/// ETag, é o cache que mantém o espelho em dia. O work item do projeto
/// (`#0`) nunca conclui.
pub fn completion(l: &Ledger, project: &Project, wi: &WorkItem) -> Result<Option<Completion>> {
    if wi.gh_number <= PROJECT_WORK_ITEM {
        return Ok(None);
    }
    let prs = l.prs_for_work_item(wi.id)?;
    if prs.iter().any(|p| p.state == PrState::Open) {
        return Ok(None);
    }
    if wi.status == WorkItemStatus::Concluido {
        return Ok(Some(Completion {
            signal: CompletionSignal::WorkItemConcluido,
            since: wi.closed_at,
            pr: None,
        }));
    }
    // Marcado `concluido` no hub (tela Hub), que não chega ao ledger daqui.
    if let Some(since) = l.hub_done(project.id, wi.gh_number)? {
        return Ok(Some(Completion {
            signal: CompletionSignal::WorkItemConcluido,
            since,
            pr: None,
        }));
    }
    // Do mais recente para o mais antigo: vale o último PR da branch.
    if let Some(pr) = prs.first() {
        let signal = match pr.state {
            PrState::Merged => CompletionSignal::PrMesclado,
            PrState::Closed => CompletionSignal::PrFechado,
            PrState::Open => unreachable!("PR aberto tratado acima"),
        };
        return Ok(Some(Completion {
            signal,
            since: Some(pr.updated_at),
            pr: Some(pr.number),
        }));
    }
    Ok(l.get_issue(project.id, wi.gh_number)?
        .filter(|i| i.state == "closed")
        .map(|i| Completion {
            signal: CompletionSignal::IssueFechada,
            since: Some(i.updated_at),
            pr: None,
        }))
}

/// Work item candidato à limpeza: o que as etapas recebem.
#[derive(Debug, Clone)]
pub struct CleanupTarget {
    pub project: Project,
    pub work_item: WorkItem,
    pub title: Option<String>,
    /// `None` = em andamento, com o worktree inativo: só a etapa `target/`.
    pub completion: Option<Completion>,
    /// Última atividade de qualquer sessão do work item (ms; o fim, se
    /// parada): base de `min_idle_hours`. No inativo, a do worktree
    /// ([`worktree_activity`]).
    pub last_activity_at: Option<i64>,
    /// Sessões vivas do work item (ids), para o aviso "issue concluída,
    /// sessão ainda aberta" (#285).
    pub live_sessions: Vec<String>,
}

/// O que cada etapa recebe ao avaliar e executar.
pub struct StepContext<'a> {
    pub sessions: &'a Sessions,
    /// Sync com o GitHub, para quem precisa da API (branch remota, #286);
    /// `None` = desativado.
    pub github: Option<&'a GithubSync>,
    /// GitLab pelo glab (branch remota dos projetos GitLab, #350); `None` =
    /// desativado.
    pub gitlab: Option<&'a dyn RemoteBranches>,
    pub config: &'a CleanupConfig,
    pub now_ms: i64,
    pub target: &'a CleanupTarget,
}

impl StepContext<'_> {
    /// A forja do projeto do work item, se configurada: o GitHub ou o
    /// GitLab.
    pub fn remote(&self) -> Option<&dyn RemoteBranches> {
        match self.target.project.forge.kind() {
            ct_core::ForgeKind::Github => self.github.map(|g| g as &dyn RemoteBranches),
            ct_core::ForgeKind::Gitlab => self.gitlab,
        }
    }
}

/// Resultado de uma etapa executada.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct StepOutcome {
    /// Espaço liberado, em bytes.
    pub freed_bytes: u64,
    /// O que saiu.
    pub removed: Vec<String>,
    /// O que ficou e por quê (ex.: "sessão abc viva").
    pub kept: Vec<String>,
}

/// Uma etapa da limpeza (épico #284). Implementações: sessões (#285),
/// worktree e `target/` (#265), branch local e remota (#286).
///
/// Contrato:
/// - `plan` não muda nada (disco, ledger, GitHub) e devolve `feita` quando
///   não há o que limpar, `bloqueada` com o motivo quando algo impede, e
///   `pendente` com o espaço estimado e o que sairia.
/// - `run` só é chamado logo depois de um `plan` pendente (reavaliado na
///   hora, com as etapas anteriores já feitas), nunca em dry-run. Na dúvida,
///   não apaga: devolve o que ficou em `kept`, ou um erro.
pub trait CleanupStep: Send + Sync {
    fn kind(&self) -> StepKind;
    fn plan(&self, ctx: &StepContext<'_>) -> Result<StepPlan>;
    fn run(&self, ctx: &StepContext<'_>) -> Result<StepOutcome>;
}

/// Um work item no plano, com as etapas na ordem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CleanupItem {
    pub work_item_id: i64,
    pub project_id: i64,
    pub owner: String,
    pub repo: String,
    pub gh_number: i64,
    /// Título da issue no espelho do GitHub, se já sincronizada.
    pub title: Option<String>,
    pub branch: Option<String>,
    pub worktree_path: Option<String>,
    /// `None` = work item em andamento com o worktree inativo há
    /// `cleanup.target_idle_days`: só a etapa `target/` (#265).
    pub completion: Option<Completion>,
    /// Última atividade conhecida (ms): das sessões, ou do worktree no
    /// item inativo.
    #[serde(default)]
    pub last_activity_at: Option<i64>,
    pub steps: Vec<StepPlan>,
    /// Sessões vivas do work item concluído (ids): nunca são encerradas sem
    /// ação explícita (#285); a UI oferece "Encerrar" para cada uma.
    #[serde(default)]
    pub live_sessions: Vec<String>,
}

impl CleanupItem {
    /// Alguma etapa pendente.
    pub fn has_pending(&self) -> bool {
        self.steps.iter().any(|s| s.state == StepState::Pendente)
    }

    /// Espaço estimado das etapas pendentes.
    pub fn pending_bytes(&self) -> u64 {
        self.steps
            .iter()
            .filter(|s| s.state == StepState::Pendente)
            .map(|s| s.bytes)
            .sum()
    }
}

/// Plano de limpeza: os work items concluídos com alguma etapa por fazer
/// (pendente ou bloqueada) e os em andamento com `target/` inativo.
/// Montá-lo nunca apaga nada.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CleanupPlan {
    /// Quando foi montado (ms).
    pub checked_at: i64,
    pub items: Vec<CleanupItem>,
    /// Work items com alguma etapa pendente.
    pub pending_items: u64,
    /// Espaço estimado das etapas pendentes, em bytes.
    pub pending_bytes: u64,
}

impl CleanupPlan {
    fn new(checked_at: i64, items: Vec<CleanupItem>) -> Self {
        let pending: Vec<&CleanupItem> = items.iter().filter(|i| i.has_pending()).collect();
        CleanupPlan {
            checked_at,
            pending_items: pending.len() as u64,
            pending_bytes: pending.iter().map(|i| i.pending_bytes()).sum(),
            items,
        }
    }

    /// Resumo que vai ao hub.
    pub fn summary(&self) -> ct_ledger::CleanupSummary {
        ct_ledger::CleanupSummary {
            checked_at: self.checked_at,
            items: self.pending_items as i64,
            bytes: i64::try_from(self.pending_bytes).unwrap_or(i64::MAX),
        }
    }
}

/// O que executar: o plano inteiro, um work item ou uma etapa dele.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CleanupScope {
    /// `None` = todos os work items do plano.
    pub work_item_id: Option<i64>,
    /// `None` = todas as etapas (exige `work_item_id`).
    pub step: Option<StepKind>,
}

/// Resultado de uma etapa numa execução.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum StepResult {
    /// Executada.
    Feita,
    /// Seria executada (dry-run).
    Simulada,
    /// Não rodou: bloqueada (`reason`), ou uma etapa anterior não terminou.
    Bloqueada,
    /// Rodou e falhou (`reason`); as seguintes não rodam.
    Falhou,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct StepRun {
    pub kind: StepKind,
    pub result: StepResult,
    pub reason: Option<String>,
    /// Espaço liberado (`feita`) ou estimado (`simulada`), em bytes.
    pub bytes: u64,
    /// O que saiu (`feita`) ou sairia (`simulada`).
    pub removed: Vec<String>,
    /// O que ficou e por quê.
    pub kept: Vec<String>,
}

impl StepRun {
    fn blocked(kind: StepKind, reason: Option<String>) -> Self {
        StepRun {
            kind,
            result: StepResult::Bloqueada,
            reason,
            bytes: 0,
            removed: Vec::new(),
            kept: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ItemRun {
    pub work_item_id: i64,
    pub owner: String,
    pub repo: String,
    pub gh_number: i64,
    /// Só as etapas que tinham o que fazer (as já feitas ficam de fora).
    pub steps: Vec<StepRun>,
}

/// Resultado de `cleanup.run`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct CleanupRun {
    pub started_at: i64,
    pub finished_at: i64,
    pub dry_run: bool,
    pub scope: CleanupScope,
    pub items: Vec<ItemRun>,
    /// Espaço liberado (ou estimado, no dry-run), em bytes.
    pub freed_bytes: u64,
}

impl CleanupRun {
    /// Etapas com o resultado `r`.
    pub fn count(&self, r: StepResult) -> usize {
        self.items
            .iter()
            .flat_map(|i| &i.steps)
            .filter(|s| s.result == r)
            .count()
    }
}

/// Monta e executa o plano de limpeza com as etapas registradas, na ordem
/// de [`StepKind::ORDER`].
pub struct Cleaner {
    steps: Vec<Box<dyn CleanupStep>>,
    config: CleanupConfig,
    github: Option<Arc<GithubSync>>,
    gitlab: Option<Arc<dyn RemoteBranches>>,
}

impl Cleaner {
    /// Com as etapas dadas, reordenadas por [`StepKind::ORDER`].
    pub fn new(config: CleanupConfig, mut steps: Vec<Box<dyn CleanupStep>>) -> Self {
        steps.sort_by_key(|s| s.kind());
        Cleaner {
            steps,
            config,
            github: None,
            gitlab: None,
        }
    }

    /// As etapas do épico #284: sessões ([`SessionsStep`], #285), worktree
    /// e `target/` ([`WorktreeStep`], [`TargetStep`], #265), branch local e
    /// remota ([`BranchLocalStep`], [`BranchRemoteStep`], #286).
    pub fn standard(config: CleanupConfig) -> Self {
        Self::new(
            config,
            StepKind::ORDER
                .into_iter()
                .map(|k| match k {
                    StepKind::Sessoes => Box::new(SessionsStep) as Box<dyn CleanupStep>,
                    StepKind::Worktree => Box::new(WorktreeStep),
                    StepKind::Target => Box::new(TargetStep),
                    StepKind::BranchLocal => Box::new(BranchLocalStep),
                    StepKind::BranchRemota => Box::new(BranchRemoteStep),
                })
                .collect(),
        )
    }

    pub fn with_github(mut self, github: Option<Arc<GithubSync>>) -> Self {
        self.github = github;
        self
    }

    /// GitLab dos projetos com forja GitLab ([`crate::gitlab::GitlabRemote`]).
    pub fn with_gitlab(mut self, gitlab: Option<Arc<dyn RemoteBranches>>) -> Self {
        self.gitlab = gitlab;
        self
    }

    pub fn config(&self) -> &CleanupConfig {
        &self.config
    }

    /// Work items concluídos de todos os projetos (inclusive os
    /// desatribuídos, cujo clone ficou no disco), sem o gestor.
    pub fn targets(&self, sessions: &Sessions) -> Result<Vec<CleanupTarget>> {
        Ok(self.collect(sessions)?.0)
    }

    /// Os concluídos ([`Self::targets`]) e depois os em andamento com o
    /// `target/` de um worktree inativo (só a etapa `target/`).
    pub fn all_targets(&self, sessions: &Sessions, now: i64) -> Result<Vec<CleanupTarget>> {
        let (mut done, open) = self.collect(sessions)?;
        done.extend(crate::cleanup_worktree::inactive_targets(
            sessions,
            open,
            self.config.target_idle_days,
            now,
        )?);
        Ok(done)
    }

    /// (concluídos, em andamento com worktree), sem o work item do projeto.
    fn collect(&self, sessions: &Sessions) -> Result<(Vec<CleanupTarget>, Vec<CleanupTarget>)> {
        let l = sessions.ledger();
        let (mut out, mut open) = (Vec::new(), Vec::new());
        for project in l.list_all_projects()? {
            let titles: std::collections::HashMap<i64, String> = l
                .list_issues(project.id)?
                .into_iter()
                .map(|i| (i.gh_number, i.title))
                .collect();
            for wi in l.list_work_items(project.id)? {
                let c = completion(&l, &project, &wi)?;
                let has_worktree = wi
                    .worktree_path
                    .as_deref()
                    .is_some_and(|p| !p.trim().is_empty());
                if c.is_none() && (wi.gh_number <= PROJECT_WORK_ITEM || !has_worktree) {
                    continue;
                }
                let sessions = l.list_sessions(wi.id)?;
                // Da sessão parada vale o fim (`ended_at`): arquivar (a etapa
                // "sessões", #285) muda o `last_event_at` e não é atividade.
                let last_activity_at = sessions
                    .iter()
                    .map(|s| match s.ended_at {
                        Some(end) if !s.state.is_live() => end,
                        _ => s.last_event_at,
                    })
                    .max();
                let live_sessions = sessions
                    .iter()
                    .filter(|s| s.state.is_live())
                    .map(|s| s.id.clone())
                    .collect();
                let t = CleanupTarget {
                    title: titles.get(&wi.gh_number).cloned(),
                    project: project.clone(),
                    work_item: wi,
                    completion: c,
                    last_activity_at,
                    live_sessions,
                };
                if c.is_some() {
                    out.push(t);
                } else {
                    open.push(t);
                }
            }
        }
        Ok((out, open))
    }

    /// Recente demais para limpar (`min_age_hours`, `min_idle_hours`):
    /// o motivo do bloqueio. `0` desliga o mínimo: o `last_event_at` é
    /// estritamente crescente e pode ficar 1 ms à frente do `now` (#369).
    fn too_recent(&self, t: &CleanupTarget, now: i64) -> Option<String> {
        let hours = |h: u64| i64::try_from(h.saturating_mul(3_600_000)).unwrap_or(i64::MAX);
        let age = hours(self.config.min_age_hours);
        if let Some((since, c)) = t.completion.and_then(|c| Some((c.since?, c))) {
            if age > 0 && now.saturating_sub(since) < age {
                return Some(format!(
                    "{} há menos de {} h (cleanup.min_age_hours)",
                    c.signal.label_for(t.project.forge.kind()),
                    self.config.min_age_hours
                ));
            }
        }
        let idle = hours(self.config.min_idle_hours);
        if let Some(at) = t.last_activity_at {
            if idle > 0 && now.saturating_sub(at) < idle {
                return Some(format!(
                    "sessão ativa há menos de {} h (cleanup.min_idle_hours)",
                    self.config.min_idle_hours
                ));
            }
        }
        None
    }

    /// As etapas que valem para o work item: todas no concluído, só a
    /// `target/` no em andamento.
    fn steps_for<'a>(&'a self, t: &CleanupTarget) -> impl Iterator<Item = &'a dyn CleanupStep> {
        let only_target = t.completion.is_none();
        self.steps
            .iter()
            .map(|s| s.as_ref())
            .filter(move |s| !only_target || s.kind() == StepKind::Target)
    }

    fn ctx<'a>(
        &'a self,
        sessions: &'a Sessions,
        t: &'a CleanupTarget,
        now: i64,
    ) -> StepContext<'a> {
        StepContext {
            sessions,
            github: self.github.as_deref(),
            gitlab: self.gitlab.as_deref(),
            config: &self.config,
            now_ms: now,
            target: t,
        }
    }

    /// Avalia as etapas de um work item, na ordem: depois de uma bloqueada,
    /// as pendentes seguintes também ficam bloqueadas.
    fn plan_target(&self, sessions: &Sessions, t: &CleanupTarget, now: i64) -> CleanupItem {
        let ctx = self.ctx(sessions, t, now);
        let recent = self.too_recent(t, now);
        let mut held: Option<StepKind> = None;
        let mut steps = Vec::with_capacity(self.steps.len());
        for step in self.steps_for(t) {
            let mut p = match step.plan(&ctx) {
                Ok(p) => p,
                Err(e) => StepPlan::bloqueada(step.kind(), format!("falha ao avaliar: {e}")),
            };
            if let Some(r) = &recent {
                p = p.block(r.clone());
            }
            if let Some(prev) = held.filter(|&h| !independent(p.kind, h)) {
                p = p.block(format!("aguarda a etapa {}", prev.label()));
            }
            if p.state == StepState::Bloqueada && held.is_none() {
                held = Some(p.kind);
            }
            steps.push(p);
        }
        let (wi, p) = (&t.work_item, &t.project);
        CleanupItem {
            work_item_id: wi.id,
            project_id: p.id,
            owner: p.gh_owner.clone(),
            repo: p.gh_repo.clone(),
            gh_number: wi.gh_number,
            title: t.title.clone(),
            branch: wi.branch.clone(),
            worktree_path: wi.worktree_path.clone(),
            completion: t.completion,
            last_activity_at: t.last_activity_at,
            steps,
            live_sessions: t.live_sessions.clone(),
        }
    }

    /// Monta o plano sem apagar nada. Ficam de fora os work items sem nada
    /// a fazer (todas as etapas feitas).
    pub fn plan(&self, sessions: &Sessions, now: i64) -> Result<CleanupPlan> {
        let items = self
            .all_targets(sessions, now)?
            .iter()
            .map(|t| self.plan_target(sessions, t, now))
            .filter(|i| i.steps.iter().any(|s| s.state != StepState::Feita))
            .collect();
        Ok(CleanupPlan::new(now, items))
    }

    /// Executa as etapas pendentes do escopo, na ordem, reavaliando cada uma
    /// na hora. Bloqueada ou com falha, a etapa segura as seguintes do mesmo
    /// work item. Com `dry_run`, só relata o que faria.
    pub fn run(
        &self,
        sessions: &Sessions,
        scope: CleanupScope,
        dry_run: bool,
        now: i64,
    ) -> Result<CleanupRun> {
        if scope.step.is_some() && scope.work_item_id.is_none() {
            return Err(SessionsError::Invalid(
                "a etapa exige o work item (work_item_id)".into(),
            ));
        }
        let mut targets = self.all_targets(sessions, now)?;
        if let Some(id) = scope.work_item_id {
            targets.retain(|t| t.work_item.id == id);
            if targets.is_empty() {
                return Err(SessionsError::Invalid(format!(
                    "work item {id} não está concluído nem tem target/ inativo (nada a limpar)"
                )));
            }
        }
        let mut items = Vec::new();
        for t in &targets {
            let steps = self.run_target(sessions, t, scope.step, dry_run, now);
            if !steps.is_empty() {
                items.push(ItemRun {
                    work_item_id: t.work_item.id,
                    owner: t.project.gh_owner.clone(),
                    repo: t.project.gh_repo.clone(),
                    gh_number: t.work_item.gh_number,
                    steps,
                });
            }
        }
        let freed_bytes = items
            .iter()
            .flat_map(|i| &i.steps)
            .filter(|s| matches!(s.result, StepResult::Feita | StepResult::Simulada))
            .map(|s| s.bytes)
            .sum();
        Ok(CleanupRun {
            started_at: now,
            finished_at: ct_core::now_ms().max(now),
            dry_run,
            scope,
            items,
            freed_bytes,
        })
    }

    fn run_target(
        &self,
        sessions: &Sessions,
        t: &CleanupTarget,
        only: Option<StepKind>,
        dry_run: bool,
        now: i64,
    ) -> Vec<StepRun> {
        let ctx = self.ctx(sessions, t, now);
        let recent = self.too_recent(t, now);
        let mut out = Vec::new();
        // Etapa bloqueada que segura as seguintes (salvo [`independent`]).
        let mut held: Option<StepKind> = None;
        for step in self.steps_for(t) {
            let kind = step.kind();
            if only.is_some_and(|k| kind > k) {
                break;
            }
            if held.is_some_and(|h| !independent(kind, h)) {
                break;
            }
            let mut p = match step.plan(&ctx) {
                Ok(p) => p,
                Err(e) => StepPlan::bloqueada(kind, format!("falha ao avaliar: {e}")),
            };
            if let Some(r) = &recent {
                p = p.block(r.clone());
            }
            let wanted = only.is_none_or(|k| k == kind);
            match p.state {
                StepState::Feita => continue,
                // Uma etapa anterior à pedida não terminou: a pedida não roda
                // (salvo se não depende dela).
                _ if !wanted => {
                    let k = only.expect("só há etapa não pedida com `only`");
                    if p.state == StepState::Bloqueada && independent(k, kind) {
                        held = Some(kind);
                        continue;
                    }
                    out.push(StepRun::blocked(
                        k,
                        Some(format!("aguarda a etapa {}", kind.label())),
                    ));
                    break;
                }
                StepState::Bloqueada => {
                    out.push(StepRun::blocked(kind, p.reason));
                    held.get_or_insert(kind);
                }
                StepState::Pendente if dry_run => out.push(StepRun {
                    kind,
                    result: StepResult::Simulada,
                    reason: None,
                    bytes: p.bytes,
                    removed: p.targets,
                    kept: Vec::new(),
                }),
                StepState::Pendente => match step.run(&ctx) {
                    Ok(o) => out.push(StepRun {
                        kind,
                        result: StepResult::Feita,
                        reason: None,
                        bytes: o.freed_bytes,
                        removed: o.removed,
                        kept: o.kept,
                    }),
                    Err(e) => {
                        out.push(StepRun {
                            kind,
                            result: StepResult::Falhou,
                            reason: Some(e.to_string()),
                            bytes: 0,
                            removed: Vec::new(),
                            kept: Vec::new(),
                        });
                        break;
                    }
                },
            }
        }
        out
    }
}

/// `kind` roda mesmo com `held` bloqueada: só a `target/` com o worktree
/// bloqueado (apagar o `target/` de um worktree que fica não perde
/// trabalho). As demais etapas seguem a ordem estrita.
fn independent(kind: StepKind, held: StepKind) -> bool {
    kind == StepKind::Target && held == StepKind::Worktree
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeClaude, TestEnv};
    use ct_core::WorkItemKind;
    use ct_ledger::{GithubSnapshot, IssueRow, PrRow};
    use std::sync::Mutex;

    const H: i64 = 3_600_000;

    /// Etapa de teste: estado fixo no `plan`, e conta as execuções.
    struct Fake {
        kind: StepKind,
        plan: StepPlan,
        runs: Arc<Mutex<Vec<(StepKind, i64)>>>,
        fail: bool,
    }

    impl CleanupStep for Fake {
        fn kind(&self) -> StepKind {
            self.kind
        }
        fn plan(&self, _: &StepContext<'_>) -> Result<StepPlan> {
            Ok(self.plan.clone())
        }
        fn run(&self, ctx: &StepContext<'_>) -> Result<StepOutcome> {
            self.runs
                .lock()
                .unwrap()
                .push((self.kind, ctx.target.work_item.gh_number));
            if self.fail {
                return Err(SessionsError::Invalid("disco ocupado".into()));
            }
            Ok(StepOutcome {
                freed_bytes: self.plan.bytes,
                removed: self.plan.targets.clone(),
                kept: Vec::new(),
            })
        }
    }

    type Runs = Arc<Mutex<Vec<(StepKind, i64)>>>;

    fn fake(kind: StepKind, plan: StepPlan, runs: &Runs) -> Box<dyn CleanupStep> {
        Box::new(Fake {
            kind,
            plan,
            runs: Arc::clone(runs),
            fail: false,
        })
    }

    fn pend(kind: StepKind, bytes: u64) -> StepPlan {
        StepPlan::pendente(kind, bytes, vec![format!("/x/{kind}")])
    }

    fn sem_idade() -> CleanupConfig {
        CleanupConfig {
            min_age_hours: 0,
            min_idle_hours: 0,
            ..CleanupConfig::default()
        }
    }

    /// Work item `n` concluído agora (status `concluido`).
    fn concluido(env: &TestEnv, n: i64) -> i64 {
        let l = env.sessions.ledger();
        let wi = l
            .ensure_work_item(env.project_id, n, WorkItemKind::Issue)
            .unwrap();
        l.set_work_item_status(wi.id, WorkItemStatus::Concluido, "teste")
            .unwrap();
        wi.id
    }

    fn snapshot(env: &TestEnv, issues: Vec<IssueRow>, prs: Vec<PrRow>) {
        env.sessions
            .ledger()
            .apply_github(&GithubSnapshot {
                project_id: env.project_id,
                issues,
                prs,
                etags: vec![],
                synced_at: ct_core::now_ms(),
            })
            .unwrap();
    }

    fn issue(n: i64, state: &str) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("Issue {n}"),
            state: state.into(),
            labels: vec![],
            parent_number: None,
            has_sub_issues: false,
            url: String::new(),
            updated_at: 1_000,
            sprints: Vec::new(),
        }
    }

    fn pr(n: i64, wi: i64, state: PrState) -> PrRow {
        PrRow {
            number: n,
            work_item_id: wi,
            head: format!("feat/{wi}"),
            base: "develop".into(),
            state,
            url: String::new(),
            updated_at: 2_000 + n,
            head_sha: None,
        }
    }

    #[test]
    fn deteccao_de_conclusao() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let (aberta, fechada, mesclado, fechado, aberto, concl) = {
            let l = env.sessions.ledger();
            let mk = |n| {
                l.ensure_work_item(env.project_id, n, WorkItemKind::Issue)
                    .unwrap()
                    .id
            };
            (mk(1), mk(2), mk(3), mk(4), mk(5), mk(6))
        };
        concluido(&env, 6);
        snapshot(
            &env,
            vec![issue(1, "open"), issue(2, "closed"), issue(5, "closed")],
            vec![
                pr(30, mesclado, PrState::Merged),
                pr(40, fechado, PrState::Closed),
                pr(50, aberto, PrState::Open),
            ],
        );
        let l = env.sessions.ledger();
        let p = l.get_project(env.project_id).unwrap();
        let c = |id: i64| completion(&l, &p, &l.get_work_item(id).unwrap()).unwrap();
        assert_eq!(c(aberta), None);
        let f = c(fechada).unwrap();
        assert_eq!(
            (f.signal, f.since),
            (CompletionSignal::IssueFechada, Some(1_000))
        );
        let m = c(mesclado).unwrap();
        // O PR mesclado também leva o status a `concluido` (gatilho do sync).
        assert!(matches!(
            m.signal,
            CompletionSignal::PrMesclado | CompletionSignal::WorkItemConcluido
        ));
        let f = c(fechado).unwrap();
        assert_eq!((f.signal, f.pr), (CompletionSignal::PrFechado, Some(40)));
        // Issue fechada com PR aberto: o trabalho continua.
        assert_eq!(c(aberto), None);
        assert_eq!(
            c(concl).unwrap().signal,
            CompletionSignal::WorkItemConcluido
        );
        // O work item do projeto (#0) nunca conclui.
        let zero = l
            .ensure_work_item(env.project_id, PROJECT_WORK_ITEM, WorkItemKind::Issue)
            .unwrap();
        l.set_work_item_status(zero.id, WorkItemStatus::Concluido, "t")
            .unwrap();
        assert_eq!(c(zero.id), None);
    }

    /// `concluido` marcado no hub (cache de `/sync/done`, #298) conclui o
    /// work item, mas um PR aberto da branch ainda vence.
    #[test]
    fn conclusao_marcada_no_hub() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let (sem_pr, com_pr_aberto) = {
            let l = env.sessions.ledger();
            let mk = |n| {
                l.ensure_work_item(env.project_id, n, WorkItemKind::Issue)
                    .unwrap()
                    .id
            };
            (mk(1), mk(2))
        };
        snapshot(&env, vec![], vec![pr(20, com_pr_aberto, PrState::Open)]);
        let l = env.sessions.ledger();
        let p = l.get_project(env.project_id).unwrap();
        let c = |id: i64| completion(&l, &p, &l.get_work_item(id).unwrap()).unwrap();
        assert_eq!(c(sem_pr), None);
        let (owner, repo) = (p.gh_owner.clone(), p.gh_repo.clone());
        let done = |numbers: &[i64]| ct_ledger::DoneResponse {
            items: numbers
                .iter()
                .map(|&number| ct_ledger::ConfigDone {
                    owner: owner.clone(),
                    repo: repo.clone(),
                    number,
                    since: Some(5_000),
                })
                .collect(),
        };
        l.apply_hub_done(&done(&[1, 2])).unwrap();
        let h = c(sem_pr).unwrap();
        assert_eq!(
            (h.signal, h.since),
            (CompletionSignal::WorkItemConcluido, Some(5_000))
        );
        assert_eq!(c(com_pr_aberto), None);
        // O cache é substituído: sai do conjunto, sai da conclusão.
        l.apply_hub_done(&done(&[])).unwrap();
        assert_eq!(c(sem_pr), None);
    }

    #[test]
    fn plano_padrao_sem_nada_a_limpar_fica_vazio() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let id = concluido(&env, 7);
        // Branch que nunca existiu no clone nem em `origin`.
        env.sessions
            .ledger()
            .set_work_item_branch(id, "feat/7-x", "")
            .unwrap();
        let c = Cleaner::standard(sem_idade());
        let now = ct_core::now_ms();
        // Sem sessões, worktree nem branch, todas as etapas estão feitas e
        // o work item fica fora do plano.
        let t = c.targets(&env.sessions).unwrap();
        assert_eq!(t.len(), 1);
        let it = c.plan_target(&env.sessions, &t[0], now);
        assert_eq!(
            it.steps.iter().map(|s| s.kind).collect::<Vec<_>>(),
            StepKind::ORDER.to_vec()
        );
        assert!(
            it.steps.iter().all(|s| s.state == StepState::Feita),
            "{it:?}"
        );
        let plan = c.plan(&env.sessions, now).unwrap();
        assert!(plan.items.is_empty(), "{plan:?}");
        assert_eq!((plan.pending_items, plan.pending_bytes), (0, 0));
        let r = c
            .run(&env.sessions, CleanupScope::default(), false, now)
            .unwrap();
        assert_eq!(r.freed_bytes, 0);
        assert!(r.items.is_empty());
    }

    /// O bloqueio do worktree não segura a etapa `target/` (apagar só o
    /// `target/` não perde trabalho), mas segura a branch local; vale no
    /// plano, na execução inteira e na execução só da `target/`.
    #[test]
    fn worktree_bloqueado_nao_segura_o_target() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let id = concluido(&env, 1);
        let runs: Runs = Arc::default();
        let c = Cleaner::new(
            sem_idade(),
            vec![
                fake(
                    StepKind::Worktree,
                    StepPlan::bloqueada(StepKind::Worktree, "mudança não commitada"),
                    &runs,
                ),
                fake(StepKind::Target, pend(StepKind::Target, 50), &runs),
                fake(StepKind::BranchLocal, pend(StepKind::BranchLocal, 0), &runs),
            ],
        );
        let now = ct_core::now_ms();
        let plan = c.plan(&env.sessions, now).unwrap();
        let st: Vec<_> = plan.items[0].steps.iter().map(|s| s.state).collect();
        assert_eq!(
            st,
            [
                StepState::Bloqueada,
                StepState::Pendente,
                StepState::Bloqueada
            ]
        );
        assert_eq!(
            plan.items[0].steps[2].reason.as_deref(),
            Some("aguarda a etapa worktree")
        );
        assert_eq!(plan.pending_bytes, 50);
        let r = c
            .run(&env.sessions, CleanupScope::default(), false, now)
            .unwrap();
        assert_eq!(*runs.lock().unwrap(), vec![(StepKind::Target, 1)]);
        let res: Vec<_> = r.items[0]
            .steps
            .iter()
            .map(|s| (s.kind, s.result))
            .collect();
        assert_eq!(
            res,
            [
                (StepKind::Worktree, StepResult::Bloqueada),
                (StepKind::Target, StepResult::Feita)
            ]
        );
        runs.lock().unwrap().clear();
        let only = CleanupScope {
            work_item_id: Some(id),
            step: Some(StepKind::Target),
        };
        let r = c.run(&env.sessions, only, false, now).unwrap();
        assert_eq!(*runs.lock().unwrap(), vec![(StepKind::Target, 1)]);
        assert_eq!(r.items[0].steps.len(), 1);
        // A branch local, pedida sozinha, espera o worktree.
        let branch = CleanupScope {
            work_item_id: Some(id),
            step: Some(StepKind::BranchLocal),
        };
        runs.lock().unwrap().clear();
        let r = c.run(&env.sessions, branch, false, now).unwrap();
        assert!(!runs
            .lock()
            .unwrap()
            .iter()
            .any(|(k, _)| *k == StepKind::BranchLocal));
        let last = r.items[0].steps.last().unwrap();
        assert_eq!(
            (last.kind, last.result),
            (StepKind::BranchLocal, StepResult::Bloqueada)
        );
    }

    #[test]
    fn ordem_e_bloqueio_seguram_as_etapas_seguintes() {
        let env = TestEnv::new(FakeClaude::exit(0));
        concluido(&env, 1);
        let runs: Runs = Arc::default();
        let c = Cleaner::new(
            sem_idade(),
            vec![
                // Fora de ordem de propósito: o Cleaner reordena.
                fake(StepKind::BranchLocal, pend(StepKind::BranchLocal, 0), &runs),
                fake(StepKind::Sessoes, StepPlan::feita(StepKind::Sessoes), &runs),
                fake(StepKind::Worktree, pend(StepKind::Worktree, 100), &runs),
                fake(
                    StepKind::Target,
                    StepPlan::bloqueada(StepKind::Target, "mudança não commitada"),
                    &runs,
                ),
            ],
        );
        let now = ct_core::now_ms();
        let plan = c.plan(&env.sessions, now).unwrap();
        let st: Vec<_> = plan.items[0]
            .steps
            .iter()
            .map(|s| (s.kind, s.state, s.reason.clone()))
            .collect();
        assert_eq!(
            st,
            vec![
                (StepKind::Sessoes, StepState::Feita, None),
                (StepKind::Worktree, StepState::Pendente, None),
                (
                    StepKind::Target,
                    StepState::Bloqueada,
                    Some("mudança não commitada".into())
                ),
                (
                    StepKind::BranchLocal,
                    StepState::Bloqueada,
                    Some("aguarda a etapa target/".into())
                ),
            ]
        );
        assert_eq!((plan.pending_items, plan.pending_bytes), (1, 100));

        // Dry-run: relata e não executa nada.
        let r = c
            .run(&env.sessions, CleanupScope::default(), true, now)
            .unwrap();
        assert!(runs.lock().unwrap().is_empty());
        assert_eq!(r.freed_bytes, 100);
        assert_eq!(r.items[0].steps[0].result, StepResult::Simulada);
        assert_eq!(r.items[0].steps[1].result, StepResult::Bloqueada);
        assert_eq!(r.items[0].steps.len(), 2);

        // Execução: só a pendente roda; a bloqueada para o work item.
        let r = c
            .run(&env.sessions, CleanupScope::default(), false, now)
            .unwrap();
        assert_eq!(*runs.lock().unwrap(), vec![(StepKind::Worktree, 1)]);
        assert_eq!(r.freed_bytes, 100);
        assert_eq!(r.count(StepResult::Feita), 1);
        assert_eq!(r.count(StepResult::Bloqueada), 1);
    }

    #[test]
    fn falha_para_o_work_item() {
        let env = TestEnv::new(FakeClaude::exit(0));
        concluido(&env, 1);
        let runs: Runs = Arc::default();
        let c = Cleaner::new(
            sem_idade(),
            vec![
                Box::new(Fake {
                    kind: StepKind::Sessoes,
                    plan: pend(StepKind::Sessoes, 1),
                    runs: Arc::clone(&runs),
                    fail: true,
                }),
                fake(StepKind::Worktree, pend(StepKind::Worktree, 5), &runs),
            ],
        );
        let r = c
            .run(
                &env.sessions,
                CleanupScope::default(),
                false,
                ct_core::now_ms(),
            )
            .unwrap();
        assert_eq!(*runs.lock().unwrap(), vec![(StepKind::Sessoes, 1)]);
        let s = &r.items[0].steps;
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].result, StepResult::Falhou);
        assert_eq!(s[0].reason.as_deref(), Some("disco ocupado"));
        assert_eq!(r.freed_bytes, 0);
    }

    #[test]
    fn escopo_por_item_e_por_etapa() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let a = concluido(&env, 1);
        concluido(&env, 2);
        let runs: Runs = Arc::default();
        let c = Cleaner::new(
            sem_idade(),
            vec![
                fake(StepKind::Sessoes, pend(StepKind::Sessoes, 1), &runs),
                fake(StepKind::Worktree, pend(StepKind::Worktree, 10), &runs),
            ],
        );
        let now = ct_core::now_ms();
        let item = |step| CleanupScope {
            work_item_id: Some(a),
            step,
        };
        // Uma etapa: as anteriores precisam estar feitas.
        let r = c
            .run(&env.sessions, item(Some(StepKind::Worktree)), false, now)
            .unwrap();
        assert!(runs.lock().unwrap().is_empty());
        let s = &r.items[0].steps[0];
        assert_eq!(
            (s.kind, s.result),
            (StepKind::Worktree, StepResult::Bloqueada)
        );
        assert_eq!(s.reason.as_deref(), Some("aguarda a etapa sessões"));
        let r = c
            .run(&env.sessions, item(Some(StepKind::Sessoes)), false, now)
            .unwrap();
        assert_eq!(*runs.lock().unwrap(), vec![(StepKind::Sessoes, 1)]);
        assert_eq!(r.freed_bytes, 1);
        runs.lock().unwrap().clear();
        // Um work item: só as etapas dele.
        c.run(&env.sessions, item(None), false, now).unwrap();
        assert_eq!(
            *runs.lock().unwrap(),
            vec![(StepKind::Sessoes, 1), (StepKind::Worktree, 1)]
        );
        // Etapa sem work item, ou work item não concluído: erro.
        let sem_item = CleanupScope {
            work_item_id: None,
            step: Some(StepKind::Sessoes),
        };
        assert!(c.run(&env.sessions, sem_item, false, now).is_err());
        let aberto = env
            .sessions
            .ledger()
            .ensure_work_item(env.project_id, 9, WorkItemKind::Issue)
            .unwrap()
            .id;
        let e = c
            .run(
                &env.sessions,
                CleanupScope {
                    work_item_id: Some(aberto),
                    step: None,
                },
                false,
                now,
            )
            .unwrap_err();
        assert!(e.to_string().contains("não está concluído"), "{e}");
    }

    #[test]
    fn idades_minimas_bloqueiam_o_recente() {
        let env = TestEnv::new(FakeClaude::exit(0));
        concluido(&env, 1);
        let runs: Runs = Arc::default();
        let c = Cleaner::new(
            CleanupConfig::default(),
            vec![fake(
                StepKind::Worktree,
                pend(StepKind::Worktree, 10),
                &runs,
            )],
        );
        let now = ct_core::now_ms();
        let plan = c.plan(&env.sessions, now).unwrap();
        let s = &plan.items[0].steps[0];
        assert_eq!(s.state, StepState::Bloqueada);
        assert!(
            s.reason
                .as_deref()
                .unwrap()
                .contains("cleanup.min_age_hours"),
            "{s:?}"
        );
        // O que sairia continua no plano (o motivo diz o que falta).
        assert_eq!(s.targets, vec!["/x/worktree".to_string()]);
        c.run(&env.sessions, CleanupScope::default(), false, now)
            .unwrap();
        assert!(runs.lock().unwrap().is_empty());
        // Passadas as 24 h, fica pendente.
        let plan = c.plan(&env.sessions, now + 25 * H).unwrap();
        assert_eq!(plan.items[0].steps[0].state, StepState::Pendente);
    }

    #[test]
    fn bytes_para_pessoas() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1,5 KB");
        assert_eq!(human_bytes(7_086_696_038), "6,6 GB");
    }

    #[test]
    fn etapa_por_nome() {
        for k in StepKind::ORDER {
            assert_eq!(k.as_str().parse::<StepKind>().unwrap(), k);
            assert_eq!(
                serde_json::to_value(k).unwrap(),
                serde_json::Value::String(k.as_str().into())
            );
        }
        assert!("lixo".parse::<StepKind>().is_err());
    }
}
