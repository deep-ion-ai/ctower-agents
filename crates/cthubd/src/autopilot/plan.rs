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
//! - **Sessão autônoma ociosa:** a sessão autônoma ([`IdleSession`]) ociosa
//!   num ambiente autônomo online é encerrada (`session.kill`, retomável
//!   como qualquer encerrada) quando a issue já tem PR mesclado ou está em
//!   revisão (ou tem PR aberto contra outra base que não a branch de
//!   integração), depois de [`DONE_GRACE_MS`] (`pr_aberto`), ou, sem PR,
//!   depois de `idle_timeout_min` (`sessao_ociosa`), se a conta não está com
//!   a janela esgotada (aí ela estaciona). A estacionada nunca é encerrada,
//!   e a que terminou a issue não estaciona: não há o que continuar.
//! - **Fechamento autônomo (#184):** a sessão autônoma ociosa cuja issue
//!   tem PR aberto contra a branch de integração do projeto ([`Closing`])
//!   não encerra com o PR só aberto: o hub dirige o fechamento pelo
//!   `session.continue` com texto (ambiente a partir de
//!   [`CONTINUE_TEXT_PROTOCOL`]).
//!   - CI pendente, ou checks ainda não lidos para o head atual: espera.
//!   - Checks concluídos com falha num head ainda não enviado: manda ao
//!     agente o pedido de correção ([`feedback_text`]: instrução fixa e os
//!     dados do espelho entre delimitadores, sem logs nem corpos externos)
//!     e conta uma tentativa. Com `max_ci_attempts` tentativas já feitas,
//!     a falha seguinte bloqueia a issue (`ci_tentativas_esgotadas`).
//!   - Thread não resolvida ou review que pede mudanças de colaborador (não
//!     externo) ainda não enviada: manda o pedido de correção, sem contar
//!     tentativa.
//!   - CI verde, sem pendência de colaborador e com as reviews lidas para a
//!     versão atual do PR: pede o merge com rebase uma vez por head
//!     ([`merge_text`]). Sem nenhum check, o head conta como verde
//!     [`NO_CHECKS_GREEN_MS`] depois de o hub vê-lo lido.
//!   - Nada novo a enviar e ociosa há `idle_timeout_min` desde o último
//!     envio: bloqueia (`sem_progresso`).
//!   - Depois do merge, o PR sai de aberto e a issue vai para `em_revisao`:
//!     a regra `pr_aberto` acima encerra a sessão.
//!   - Ambiente sem o texto no `session.continue`: o comportamento anterior
//!     (`pr_aberto`), com uma recusa `env_outdated` no escopo `env:`.
//! - **Aguardando você (#184):** a sessão autônoma viva cujo work item está
//!   em `aguardando_voce` há `waiting_timeout_min` é encerrada e a issue
//!   fica `bloqueado` (`aguardando_voce`). A estacionada nunca, nem a da
//!   conta com a janela esgotada (essa estaciona).
//! - **Análise de dependências (#215):** com a janela da rodada aberta
//!   ([`super::deps::window_open`]: a faixa da política permite iniciar) e o
//!   orçamento permitindo um início, o projeto com issues na fila ganha uma
//!   sessão de análise ([`Action::StartAnalysis`], sessão do projeto, #0)
//!   antes de qualquer implementação: enquanto o planejamento não termina, o
//!   projeto fica bloqueado (`analysis`). Ela não ocupa a vaga do ambiente
//!   e conta como o início da conta no ciclo. Depois, issue nova ou alterada
//!   (impressão digital diferente da analisada) fica fora (`analysis_pending`)
//!   e o hub acorda a sessão ociosa com a lista ([`Action::ContinueAnalysis`],
//!   nunca com a conta na janela esgotada). Fim da janela, modo desligado ou
//!   ambiente sem a análise: a sessão é encerrada e a rodada fecha
//!   ([`Action::CloseAnalysis`]). A sessão que some, não inicia ou fica
//!   parada com a reanálise pendente falha ([`Action::FailAnalysis`]) e é
//!   reiniciada depois de [`ANALYSIS_RETRY_MS`]. Ambiente anterior a
//!   [`ANALYSIS_PROTOCOL`]: sem análise nem gate, só as arestas gravadas.
//! - **Dependências (#215):** issue com bloqueante que não está pronto (PR
//!   mesclado na branch de integração, ou concluído ou fechado) fica fora
//!   com `blocked_by` e os números; bloqueante fora do espelho mantém o
//!   bloqueio e vira aviso (`dependency_unknown`); as issues de um ciclo
//!   ficam fora com `dependency_cycle` (e o evento).
//! - **Execução de épico (#303):** sub-issue de um épico com execução ativa
//!   ([`ItemSnap::epic_run`]) fica fora com `epic_run`: quem a dirige é o
//!   motor do épico. Com a execução concluída ou cancelada, volta às regras
//!   normais.

use super::deps;
use ct_core::plan::budget::{BlockReason, BudgetDecision};
use ct_core::plan::{PlanView, WindowView};
use ct_core::{Priority, SessionState, SessionTemplates, WipUsage, WorkItemKind, WorkItemStatus};
use ct_hub_store::{
    AnalysisRound, AutopilotSession, AutopilotSettings, BlockerState, CloseState, DependencyEdge,
    IdleSession, ParkedSession, WaitingSession,
};
use ct_ledger::{pr_feedback_text, ChecksState, FeedbackTextOptions, PrChecks, PrFeedback, PrRow};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

/// Folga depois do `resets_at` antes de retomar: a renovação não é exata.
pub const RESUME_MARGIN_MS: i64 = 60_000;

/// Ociosa com a issue já com PR: espera isto antes de encerrar (o agente
/// pode estar fechando o turno, ou alguém olhando).
pub const DONE_GRACE_MS: i64 = 5 * 60_000;

/// `reason` do evento `ended`: a issue já tem PR aberto (ou está em
/// revisão).
pub const END_DONE: &str = "pr_aberto";
/// `reason` do evento `ended`: ociosa sem PR por mais de
/// `idle_timeout_min`.
pub const END_IDLE: &str = "sessao_ociosa";

/// `reason` do evento `blocked`: o CI falhou de novo depois das
/// `max_ci_attempts` tentativas de correção.
pub const BLOCK_ATTEMPTS: &str = "ci_tentativas_esgotadas";
/// `reason` do evento `blocked`: parada em "Aguardando você" por mais de
/// `waiting_timeout_min`.
pub const BLOCK_WAITING: &str = "aguardando_voce";
/// `reason` do evento `blocked`: ociosa com o PR na branch de integração e
/// nada novo a enviar por mais de `idle_timeout_min`.
pub const BLOCK_STALLED: &str = "sem_progresso";
/// `reason` do evento `feedback_sent`: checks falharam.
pub const FEEDBACK_CI: &str = "ci_falhou";
/// `reason` do evento `feedback_sent`: review de colaborador pendente.
pub const FEEDBACK_REVIEW: &str = "review";
/// `reason` do evento `merge_requested`.
pub const MERGE_GREEN: &str = "ci_verde";
/// `reason` da recusa no escopo `env:`: o ambiente não aceita texto no
/// `session.continue`.
pub const ENV_OUTDATED: &str = "env_outdated";

/// Versão do protocolo do ambiente com o `text` no `session.continue`.
pub const CONTINUE_TEXT_PROTOCOL: (u32, u32) = (2, 22);

/// Versão do protocolo do ambiente com a sessão de análise (#215):
/// `analysis` no `session.start_project`, o texto no `session.continue` dela
/// e a ferramenta `autopilot_set_dependencies`.
pub const ANALYSIS_PROTOCOL: (u32, u32) = (2, 24);
/// Depois de uma falha da sessão de análise, espera isto para iniciar outra.
pub const ANALYSIS_RETRY_MS: i64 = 15 * 60_000;
/// Sessão de análise ociosa com a reanálise pendente há isto: falhou.
pub const ANALYSIS_STALL_MS: i64 = 30 * 60_000;
/// Sessão de análise iniciada que o hub ainda não conhece conta como viva
/// por este tempo (o ambiente sincroniza).
pub const ANALYSIS_START_GRACE_MS: i64 = 10 * 60_000;

/// Eventos da análise (#215).
pub const EV_ANALYSIS_STARTED: &str = "analysis_started";
pub const EV_ANALYSIS_DONE: &str = "analysis_done";
pub const EV_ANALYSIS_FAILED: &str = "analysis_failed";
pub const EV_ANALYSIS_ENDED: &str = "analysis_ended";
pub const EV_DEPENDENCY_CYCLE: &str = "dependency_cycle";
pub const EV_DEPENDENCY_UNKNOWN: &str = "dependency_unknown";
pub const EV_ANALYSIS_UNSUPPORTED: &str = "analysis_unsupported";
/// `reason` do `analysis_started`: o planejamento da janela ou a reanálise.
pub const ANALYSIS_PLANNING: &str = "planejamento";
pub const ANALYSIS_REANALYSIS: &str = "reanalise";

/// Head sem nenhum check conta como verde depois disto desde que o hub o
/// viu lido (repositório sem CI). Dá tempo ao Actions de registrar os
/// checks do push.
pub const NO_CHECKS_GREEN_MS: i64 = 10 * 60_000;

/// Instrução fixa do pedido de correção, antes dos dados.
pub const FEEDBACK_REQUEST: &str = "Piloto automático do Control Tower: o PR desta issue \
precisa de correção (checks do CI falharam ou há review de colaborador pendente). Corrija, \
faça push na mesma branch e responda nas threads de review. Use as ferramentas pr_feedback \
e pr_check_log do Control Tower para ver os detalhes e os logs dos checks. Não mescle o PR \
agora: o hub pede o merge quando o CI passar.";

/// O pedido de merge: CI verde no PR `n`, base `branch`.
pub fn merge_text(n: i64, branch: &str) -> String {
    format!(
        "Piloto automático do Control Tower: CI verde no PR #{n}, sem review pendente. Mescle \
         o PR #{n} na {branch} com `gh pr merge {n} --rebase` e encerre (não mescle em outra \
         branch nem abra outro PR)."
    )
}

/// O pedido de correção do PR: [`FEEDBACK_REQUEST`] e o feedback do
/// espelho ([`pr_feedback_text`], sem logs nem corpos externos). Passando
/// de `ct_core::autonomy::CONTINUE_TEXT_MAX_BYTES`, vai só a instrução e o
/// aviso para ler pelo `pr_feedback`.
pub fn feedback_text(pr: &PrRow, checks: Option<&PrChecks>, fb: Option<&PrFeedback>) -> String {
    let full = pr_feedback_text(
        pr,
        checks,
        fb,
        &FeedbackTextOptions {
            request: Some(FEEDBACK_REQUEST),
            include_external_bodies: false,
            logs: &[],
        },
    );
    if full.len() <= ct_core::autonomy::CONTINUE_TEXT_MAX_BYTES {
        return full;
    }
    format!(
        "{FEEDBACK_REQUEST}\n\nO feedback do PR #{} passou do tamanho que cabe aqui: leia com \
         a ferramenta pr_feedback.",
        pr.number
    )
}

/// `major.minor` do protocolo do ambiente (`env.info.protocol`).
pub fn parse_protocol(v: &str) -> Option<(u32, u32)> {
    let (a, b) = v.trim().split_once('.')?;
    Some((a.parse().ok()?, b.split('.').next()?.parse().ok()?))
}

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
    /// As sessões ociosas, por id.
    pub idle: BTreeMap<String, IdleSession>,
    /// Sessões que o agendador acabou de encerrar (o ambiente ainda não
    /// sincronizou): não encerra de novo.
    pub ended: BTreeSet<String>,
    /// Sessões vivas em "Aguardando você", por id (#184).
    pub waiting: BTreeMap<String, WaitingSession>,
    /// Sessões autônomas ociosas com o PR aberto da issue contra a branch
    /// de integração do projeto, por id (#184).
    pub closing: BTreeMap<String, Closing>,
    /// Rodadas de análise abertas (#215), por projeto.
    pub analyses: BTreeMap<i64, AnalysisSnap>,
    /// Estado das sessões de análise das rodadas que o hub conhece, por id.
    pub analysis_states: BTreeMap<String, SessionState>,
}

/// A rodada de análise aberta de um projeto, com o projeto (o modo pode já
/// ter sido desligado).
#[derive(Debug, Clone, Default)]
pub struct AnalysisSnap {
    pub owner: String,
    pub repo: String,
    pub round: AnalysisRound,
}

/// O PR aberto de uma issue autônoma contra a branch de integração (#184),
/// com o que o espelho do hub sabe dele.
#[derive(Debug, Clone)]
pub struct Closing {
    pub work_item_id: i64,
    /// Branch de integração do projeto.
    pub integration: String,
    pub pr: PrRow,
    pub checks: Option<PrChecks>,
    pub feedback: Option<PrFeedback>,
    /// O que já foi enviado (padrão = nada).
    pub state: CloseState,
}

#[derive(Debug, Clone, Default)]
pub struct EnvSnap {
    pub name: String,
    pub online: bool,
    /// Marcado como autônomo (#181).
    pub autonomous: bool,
    /// Chave da conta do Claude Code do ambiente; `None` = desconhecida.
    pub account: Option<String>,
    /// O ambiente aceita texto no `session.continue` (protocolo
    /// [`CONTINUE_TEXT_PROTOCOL`] ou mais novo, #184); `None` = não se sabe
    /// (sem `env.info` neste ciclo): o fechamento espera.
    pub continue_text: Option<bool>,
    /// O ambiente roda a sessão de análise (protocolo
    /// [`ANALYSIS_PROTOCOL`] ou mais novo, #215); `None` = não se sabe: o
    /// projeto espera.
    pub analysis: Option<bool>,
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
    /// Id do projeto no hub (#215).
    pub project_id: i64,
    /// Branch de integração do modo autônomo.
    pub integration: String,
    /// Arestas de dependência do projeto, com as lápides manuais.
    pub deps: Vec<DependencyEdge>,
    /// Situação de cada bloqueante citado nas arestas ativas.
    pub blockers: BTreeMap<i64, BlockerState>,
    /// Ordem de execução sugerida pela análise (número → posição).
    pub order: BTreeMap<i64, i64>,
    /// Impressão digital de cada issue na última análise.
    pub analysed: BTreeMap<i64, String>,
    /// Templates do projeto (o `analysis` vai no prompt da análise).
    pub templates: SessionTemplates,
    /// Como a sessão de análise lê as issues (o comando do rastreador).
    pub tracker_hint: String,
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
    /// Impressão digital (`ct_hub_store::issue_fingerprint`): muda com o
    /// título, o corpo, os rótulos ou a prioridade (#215).
    pub fingerprint: String,
    /// Sub-issue presa a uma execução de épico ativa (#303).
    pub epic_run: bool,
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
    /// Encerra a sessão autônoma ociosa (`session.kill`).
    End {
        session: AutopilotSession,
        /// [`END_DONE`] ou [`END_IDLE`].
        reason: &'static str,
        message: String,
    },
    /// Manda um texto à sessão autônoma (`session.continue` com `text`) e,
    /// se o ambiente aceitar, grava `close` (#184).
    Send {
        session: AutopilotSession,
        kind: SendKind,
        close: CloseState,
        text: String,
        message: String,
    },
    /// Grava o estado do fechamento sem enviar nada (o head novo lido).
    Track(CloseState),
    /// Encerra a sessão autônoma (`session.kill`) e põe a issue em
    /// `bloqueado` com o motivo (#184).
    Block {
        session: AutopilotSession,
        work_item_id: i64,
        /// [`BLOCK_ATTEMPTS`], [`BLOCK_WAITING`] ou [`BLOCK_STALLED`].
        reason: &'static str,
        /// Motivo no status da issue.
        status_reason: String,
        message: String,
    },
    /// Inicia a sessão de análise do projeto (#215).
    StartAnalysis(AnalysisStart),
    /// Acorda a sessão de análise ociosa com as issues novas ou alteradas
    /// (`session.continue` com `text`) e grava `round` (o que foi enviado).
    ContinueAnalysis {
        round: AnalysisRound,
        owner: String,
        repo: String,
        text: String,
        message: String,
    },
    /// Fim da janela, modo desligado ou ambiente sem a análise: encerra a
    /// sessão (se houver) e apaga a rodada.
    CloseAnalysis {
        round: AnalysisRound,
        owner: String,
        repo: String,
        message: String,
    },
    /// A sessão de análise sumiu ou parou: grava `round` (sem sessão, com a
    /// falha) e, com `kill`, encerra a sessão parada.
    FailAnalysis {
        round: AnalysisRound,
        kill: Option<String>,
        owner: String,
        repo: String,
        message: String,
    },
}

/// O início da sessão de análise de um projeto (#215).
#[derive(Debug, Clone, PartialEq)]
pub struct AnalysisStart {
    pub account: String,
    pub env_id: String,
    pub owner: String,
    pub repo: String,
    /// A rodada a gravar quando o ambiente aceitar (o runner põe a sessão).
    pub round: AnalysisRound,
    pub prompt: String,
    /// [`ANALYSIS_PLANNING`] ou [`ANALYSIS_REANALYSIS`] (a sessão anterior
    /// da janela falhou depois do planejamento).
    pub reason: &'static str,
}

/// O que o texto do [`Action::Send`] pede.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendKind {
    /// Correção: [`FEEDBACK_CI`] ou [`FEEDBACK_REVIEW`].
    Feedback(&'static str),
    /// Merge com o CI verde.
    Merge,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StartItem {
    pub account: String,
    pub env_id: String,
    pub owner: String,
    pub repo: String,
    pub number: i64,
    /// Título do espelho do hub: vai no `session.start`, porque o espelho do
    /// ambiente pode ainda não ter a issue (projeto recém-atribuído).
    pub title: String,
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
    /// `None` = elegível. Senão: `epic`, `epic_run` (sub-issue de épico em
    /// execução, #303), `status`, `not_in_mirror`,
    /// `not_approved`, `no_priority`, `no_model`, `live_session` e, com as
    /// dependências (#215), `dependency_cycle`, `blocked_by` (bloqueante
    /// ainda não pronto) ou `analysis_pending` (nova ou alterada, esperando
    /// a reanálise).
    pub skip: Option<String>,
    /// Bloqueantes que ainda não estão prontos (`skip` = `blocked_by`).
    pub blocked_by: Vec<i64>,
    /// Arestas ativas da issue (prontas ou não) e as removidas à mão.
    pub dependencies: Vec<QueueDep>,
    /// Posição na ordem sugerida pela análise.
    pub order: Option<i64>,
}

/// Uma aresta da issue na fila (#215).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct QueueDep {
    pub blocker: i64,
    pub reason: String,
    /// `analysis` ou `manual`.
    pub source: String,
    /// Removida à mão (não bloqueia; a análise não a recria).
    pub removed: bool,
    pub state: BlockerState,
}

/// A rodada de análise do projeto, para o painel (#215).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AnalysisView {
    /// `planning` (planejamento inicial), `ready` (ociosa, esperando
    /// mudanças), `reanalysing` (com issues enviadas) ou `failed` (sem
    /// sessão, esperando para reiniciar).
    pub state: String,
    pub session_id: Option<String>,
    pub opened_at: i64,
    pub planned_at: Option<i64>,
    /// Issues enviadas e ainda sem resposta.
    pub pending: Vec<i64>,
    pub failed_at: Option<i64>,
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
    /// `budget:<motivos>`, `conflict`, `autonomy_error`, `analysis` (o
    /// planejamento de dependências da janela não terminou, #215),
    /// `env_busy`, `wip_limit`, `account_turn` (a conta já iniciou neste
    /// ciclo) ou `no_eligible`.
    pub blocked: Option<String>,
    /// Texto do motivo, para a pessoa.
    pub blocked_message: Option<String>,
    /// Item que inicia agora.
    pub next: Option<i64>,
    /// O início é a sonda sem leitura do plano (`next` sai mesmo com o
    /// orçamento em `no_reading`).
    pub probe: bool,
    pub queue: Vec<QueueItem>,
    /// Rodada de análise aberta (#215); `None` = nenhuma (fora da janela).
    pub analysis: Option<AnalysisView>,
    /// O ambiente não roda a análise (versão anterior): só as arestas
    /// gravadas valem.
    pub analysis_unsupported: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    pub actions: Vec<Action>,
    pub refusals: Vec<Refusal>,
    pub projects: Vec<ProjectPlan>,
    /// Avisos da análise e das dependências (#215), com o `kind` do evento
    /// (`dependency_cycle`, `dependency_unknown`, `analysis_unsupported`);
    /// gravados como as recusas, só quando mudam no escopo.
    pub notices: Vec<(&'static str, Refusal)>,
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
    } else if item.epic_run {
        Some("epic_run")
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

/// Ids das threads e reviews de colaborador (não externos) pendentes, em
/// ordem.
fn collaborator_items(fb: Option<&PrFeedback>) -> Vec<String> {
    let mut ids: Vec<String> = fb
        .map(|f| {
            f.threads
                .iter()
                .filter(|t| !t.external)
                .map(|t| t.id.clone())
                .chain(
                    f.changes_requested
                        .iter()
                        .filter(|c| !c.external)
                        .map(|c| c.id.clone()),
                )
                .collect()
        })
        .unwrap_or_default();
    ids.sort();
    ids.dedup();
    ids
}

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// O fechamento de uma sessão autônoma ociosa com o PR na branch de
/// integração (veja o módulo).
fn close_step(
    s: &Snapshot,
    x: &AutopilotSession,
    i: &IdleSession,
    c: &Closing,
    slug: &str,
) -> Vec<Action> {
    let now = s.now;
    let n = c.pr.number;
    let mut st = CloseState {
        work_item_id: c.work_item_id,
        ..c.state.clone()
    };
    // Checks ainda não lidos para o head atual: espera.
    let Some(head) = c.pr.head_sha.clone() else {
        return Vec::new();
    };
    let Some(checks) = c.checks.as_ref().filter(|k| k.head_sha == head) else {
        return Vec::new();
    };
    if st.head_sha.as_deref() != Some(head.as_str()) {
        st.head_sha = Some(head.clone());
        st.head_seen_at = Some(now);
    }
    let track = |st: CloseState| -> Vec<Action> {
        if st != c.state {
            vec![Action::Track(st)]
        } else {
            Vec::new()
        }
    };
    let seen_at = st.head_seen_at.unwrap_or(now);
    let completed = checks.completed();
    let failed = completed && checks.state == ChecksState::Failure;
    let no_checks = checks.checks.is_empty();
    let green = (completed && checks.state == ChecksState::Success)
        || (no_checks && now - seen_at >= NO_CHECKS_GREEN_MS);
    // CI pendente (ou repositório sem checks ainda no prazo): espera.
    if !failed && !green {
        return track(st);
    }
    let collab = collaborator_items(c.feedback.as_ref());
    let new_items = collab.iter().any(|id| !st.threads_sent.contains(id));
    let max = s.settings.max_ci_attempts;
    if failed && st.feedback_head.as_deref() != Some(head.as_str()) {
        if st.attempts >= max {
            let tries = if max == 0 {
                "sem reenvio (0 tentativas configuradas)".to_string()
            } else {
                format!(
                    "depois de {max} {}",
                    if max == 1 {
                        "tentativa de correção"
                    } else {
                        "tentativas de correção"
                    }
                )
            };
            return vec![Action::Block {
                session: x.clone(),
                work_item_id: c.work_item_id,
                reason: BLOCK_ATTEMPTS,
                status_reason: format!(
                    "piloto automático: o CI do PR #{n} falhou {tries} (head {})",
                    short(&head)
                ),
                message: format!(
                    "{slug}: o CI do PR #{n} falhou {tries} (head {}); a sessão foi encerrada \
                     e a issue ficou bloqueada",
                    short(&head)
                ),
            }];
        }
        st.attempts += 1;
        st.feedback_head = Some(head.clone());
        st.threads_sent = collab;
        st.last_sent_at = Some(now);
        let message = format!(
            "{slug}: o CI do PR #{n} falhou (head {}); o pedido de correção foi enviado ao \
             agente (tentativa {} de {max})",
            short(&head),
            st.attempts
        );
        return vec![Action::Send {
            session: x.clone(),
            kind: SendKind::Feedback(FEEDBACK_CI),
            close: st,
            text: feedback_text(&c.pr, Some(checks), c.feedback.as_ref()),
            message,
        }];
    }
    if new_items {
        let k = collab.len();
        st.threads_sent = collab;
        st.last_sent_at = Some(now);
        return vec![Action::Send {
            session: x.clone(),
            kind: SendKind::Feedback(FEEDBACK_REVIEW),
            close: st,
            text: feedback_text(&c.pr, Some(checks), c.feedback.as_ref()),
            message: format!(
                "{slug}: o PR #{n} tem {k} {} de colaborador pendente(s); o pedido de \
                 correção foi enviado ao agente (head {})",
                if k == 1 { "review" } else { "reviews" },
                short(&head)
            ),
        }];
    }
    let reviews_fresh = c
        .feedback
        .as_ref()
        .is_some_and(|f| f.read_at_updated == Some(c.pr.updated_at));
    if green
        && collab.is_empty()
        && reviews_fresh
        && st.merge_head.as_deref() != Some(head.as_str())
    {
        st.merge_head = Some(head.clone());
        st.last_sent_at = Some(now);
        let sem = if no_checks { " (sem checks)" } else { "" };
        return vec![Action::Send {
            session: x.clone(),
            kind: SendKind::Merge,
            close: st,
            text: merge_text(n, &c.integration),
            message: format!(
                "{slug}: CI verde no PR #{n}{sem} (head {}); o merge na {} foi pedido ao agente",
                short(&head),
                c.integration
            ),
        }];
    }
    // Nada novo a enviar: parada sem progresso depois do prazo.
    let timeout_ms = i64::from(s.settings.idle_timeout_min) * 60_000;
    let since = i.since.max(st.last_sent_at.unwrap_or(i64::MIN));
    if timeout_ms > 0 && now - since >= timeout_ms {
        let why = if failed {
            "o CI segue falhando e o agente não mandou correção"
        } else if !collab.is_empty() {
            "há review de colaborador pendente"
        } else if st.merge_head.as_deref() == Some(head.as_str()) {
            "o merge foi pedido e não aconteceu"
        } else {
            "o PR não avançou"
        };
        let min = s.settings.idle_timeout_min;
        return vec![Action::Block {
            session: x.clone(),
            work_item_id: c.work_item_id,
            reason: BLOCK_STALLED,
            status_reason: format!(
                "piloto automático: PR #{n} parado há mais de {min} min ({why})"
            ),
            message: format!(
                "{slug}: a sessão autônoma ficou ociosa por mais de {min} min com o PR #{n} \
                 parado ({why}); foi encerrada e a issue ficou bloqueada"
            ),
        }];
    }
    track(st)
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

    // 2. Encerrar ou fechar: sessões autônomas paradas em "Aguardando
    // você", com o PR na branch de integração, que terminaram a issue ou
    // pararam sem PR (veja o módulo).
    let exhausted_now = |key: &str| {
        s.accounts
            .get(key)
            .and_then(|a| a.exhausted.as_ref())
            .is_some_and(|ex| ex.resets_at > now)
    };
    let timeout_ms = i64::from(s.settings.idle_timeout_min) * 60_000;
    let waiting_ms = i64::from(s.settings.waiting_timeout_min) * 60_000;
    let mut done_ids: BTreeSet<&str> = BTreeSet::new();
    let mut outdated_envs: BTreeSet<&str> = BTreeSet::new();
    for x in &s.sessions {
        if parked_ids.contains(x.session_id.as_str())
            || !x.state.is_live()
            || s.ended.contains(&x.session_id)
        {
            continue;
        }
        let Some(e) = env(&x.env_id).filter(|e| e.autonomous) else {
            continue;
        };
        let idle = s
            .idle
            .get(&x.session_id)
            .filter(|i| i.autonomous && x.state == SessionState::Idle);
        let closing = s.closing.get(&x.session_id).filter(|_| idle.is_some());
        let new_closing = closing.is_some() && e.continue_text != Some(false);
        if idle.is_some_and(|i| i.done) && !new_closing {
            done_ids.insert(x.session_id.as_str());
        }
        if !e.online {
            continue;
        }
        let slug = format!("{}/{}#{}", x.owner, x.repo, x.gh_number);
        let exhausted = e.account.as_deref().is_some_and(exhausted_now);
        // "Aguardando você" além do prazo: bloqueia.
        if let Some(w) = s.waiting.get(&x.session_id).filter(|w| w.autonomous) {
            if waiting_ms > 0 && now - w.since >= waiting_ms && !exhausted {
                let min = s.settings.waiting_timeout_min;
                out.actions.push(Action::Block {
                    session: x.clone(),
                    work_item_id: w.work_item_id,
                    reason: BLOCK_WAITING,
                    status_reason: format!(
                        "piloto automático: sessão autônoma parada aguardando você há mais de \
                         {min} min"
                    ),
                    message: format!(
                        "{slug}: a sessão autônoma ficou parada aguardando você por mais de \
                         {min} min; foi encerrada e a issue ficou bloqueada"
                    ),
                });
                continue;
            }
        }
        let Some(i) = idle else { continue };
        // Fechamento pelo hub (#184). Conta com a janela esgotada: a
        // sessão estaciona (passo 3) em vez de receber o pedido.
        if let Some(c) = closing {
            match e.continue_text {
                Some(true) => {
                    if !exhausted {
                        out.actions.extend(close_step(s, x, i, c, &slug));
                    }
                    continue;
                }
                None => continue,
                Some(false) => {
                    if outdated_envs.insert(x.env_id.as_str()) {
                        out.refusals.push(Refusal {
                            scope: format!("env:{}", x.env_id),
                            reason: ENV_OUTDATED.into(),
                            message: format!(
                                "o ambiente autônomo {} é anterior ao protocolo {}.{}: o hub \
                                 não devolve o CI ao agente nem pede o merge; a sessão com PR \
                                 aberto é encerrada como antes (atualize o ambiente)",
                                e.name, CONTINUE_TEXT_PROTOCOL.0, CONTINUE_TEXT_PROTOCOL.1
                            ),
                            account: e.account.clone(),
                            env_id: Some(x.env_id.clone()),
                            owner: None,
                            repo: None,
                        });
                    }
                }
            }
        }
        let idle_ms = now - i.since;
        let (reason, message) = if i.done {
            if idle_ms < DONE_GRACE_MS {
                continue;
            }
            (
                END_DONE,
                format!(
                    "{slug}: a issue já tem PR; a sessão autônoma ociosa foi encerrada para \
                     liberar o ambiente (dá para retomá-la)"
                ),
            )
        } else {
            // Conta com a janela esgotada: a sessão parou pelo limite e
            // estaciona (passo 3), não encerra.
            if timeout_ms == 0 || idle_ms < timeout_ms || exhausted {
                continue;
            }
            (
                END_IDLE,
                format!(
                    "{slug}: a sessão autônoma ficou ociosa sem PR por mais de {} min e foi \
                     encerrada para liberar o ambiente (dá para retomá-la)",
                    s.settings.idle_timeout_min
                ),
            )
        };
        out.actions.push(Action::End {
            session: x.clone(),
            reason,
            message,
        });
    }

    // 3. Estacionar: sessões ociosas de ambientes autônomos numa conta com
    // política e com a janela esgotada. A que terminou a issue não
    // estaciona (encerra no passo 2).
    for x in &s.sessions {
        if parked_ids.contains(x.session_id.as_str())
            || x.state != SessionState::Idle
            || done_ids.contains(x.session_id.as_str())
        {
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

    // 4. Rodadas de análise (#215): fim da janela, modo desligado ou
    // ambiente sem a análise fecham a rodada; a sessão que sumiu, não
    // iniciou ou parou com a reanálise pendente falha.
    let decision_of = |env_id: &str| {
        account_of(env_id).and_then(|k| s.accounts.get(&k).and_then(|a| a.decision.clone()))
    };
    let mut rounds: BTreeMap<i64, AnalysisRound> = BTreeMap::new();
    for (pid, a) in &s.analyses {
        let r = &a.round;
        let slug = format!("{}/{}", a.owner, a.repo);
        let proj = s.projects.iter().find(|p| p.project_id == *pid);
        let why_close = match proj {
            None => Some("o modo autônomo do projeto foi desligado".to_string()),
            Some(p) if env(&p.env_id).and_then(|e| e.analysis) == Some(false) => {
                Some("o ambiente autônomo não roda a análise".to_string())
            }
            Some(p) if p.env_id != r.env_id => {
                Some("o ambiente do modo autônomo mudou".to_string())
            }
            Some(p) => match decision_of(&p.env_id) {
                Some(d) if deps::window_open(&d) => None,
                _ => Some("a janela da política fechou".to_string()),
            },
        };
        if let Some(why) = why_close {
            let m = if r.session_id.is_some() {
                format!("{slug}: {why}; a sessão de análise foi encerrada (as implementações em andamento continuam)")
            } else {
                format!("{slug}: {why}; a rodada de análise terminou")
            };
            out.actions.push(Action::CloseAnalysis {
                round: r.clone(),
                owner: a.owner.clone(),
                repo: a.repo.clone(),
                message: m,
            });
            continue;
        }
        let mut r = r.clone();
        if let Some(sid) = r.session_id.clone() {
            let st = s.analysis_states.get(&sid).copied();
            let started = r.started_at.unwrap_or(r.opened_at);
            let gone = match st {
                Some(st) => !st.is_live(),
                None => now - started >= ANALYSIS_START_GRACE_MS,
            };
            let exhausted = proj
                .and_then(|p| account_of(&p.env_id))
                .is_some_and(|k| exhausted_now(&k));
            let stalled = st == Some(SessionState::Idle)
                && !r.pending.is_empty()
                && !exhausted
                && r.sent_at.is_some_and(|t| now - t >= ANALYSIS_STALL_MS);
            if gone || stalled {
                let why = if stalled {
                    format!(
                        "a sessão de análise ficou parada por mais de {} min sem gravar a \
                         reanálise; foi encerrada",
                        ANALYSIS_STALL_MS / 60_000
                    )
                } else if st.is_none() {
                    "a sessão de análise não apareceu no ambiente".to_string()
                } else {
                    "a sessão de análise terminou".to_string()
                };
                let pending = if r.planned_at.is_none() {
                    " antes de terminar o planejamento"
                } else if !r.pending.is_empty() {
                    " sem gravar a reanálise"
                } else {
                    ""
                };
                r.session_id = None;
                r.pending.clear();
                r.failed_at = Some(now);
                out.actions.push(Action::FailAnalysis {
                    round: r.clone(),
                    kill: stalled.then_some(sid),
                    owner: a.owner.clone(),
                    repo: a.repo.clone(),
                    message: format!(
                        "{slug}: {why}{pending}; outra inicia em {} min",
                        ANALYSIS_RETRY_MS / 60_000
                    ),
                });
            }
        }
        rounds.insert(*pid, r);
    }

    // 5. Inícios.
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
        // Dependências (#215): arestas ativas por issue; as que esperam um
        // bloqueante não pronto formam o grafo dos ciclos.
        let gate = e.and_then(|e| e.analysis);
        let state_of = |b: i64| p.blockers.get(&b).copied().unwrap_or(BlockerState::Unknown);
        let mut waiting: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
        for d in p.deps.iter().filter(|d| !d.removed) {
            if state_of(d.blocker) != BlockerState::Ready {
                waiting.entry(d.number).or_default().insert(d.blocker);
            }
        }
        let cycle = deps::cycle_members(&waiting);
        let base: Vec<Option<&'static str>> = p
            .items
            .iter()
            .map(|i| {
                skip_of(
                    i,
                    p.approval_label.as_deref(),
                    live_item(&p.owner, &p.repo, i.number),
                )
            })
            .collect();
        let candidates: Vec<&ItemSnap> = p
            .items
            .iter()
            .zip(&base)
            .filter(|(_, b)| b.is_none())
            .map(|(i, _)| i)
            .collect();
        // Sem a análise no ambiente, nenhuma issue espera por ela.
        let unanalysed: Vec<&ItemSnap> = if gate == Some(false) {
            Vec::new()
        } else {
            candidates
                .iter()
                .copied()
                .filter(|i| p.analysed.get(&i.number) != Some(&i.fingerprint))
                .collect()
        };
        let mut unknown: Vec<String> = Vec::new();
        let queue: Vec<QueueItem> = p
            .items
            .iter()
            .zip(&base)
            .map(|(i, b)| {
                let dependencies: Vec<QueueDep> = p
                    .deps
                    .iter()
                    .filter(|d| d.number == i.number)
                    .map(|d| QueueDep {
                        blocker: d.blocker,
                        reason: d.reason.clone(),
                        source: d.source.clone(),
                        removed: d.removed,
                        state: state_of(d.blocker),
                    })
                    .collect();
                let blocked_by: Vec<i64> = waiting
                    .get(&i.number)
                    .map(|bs| bs.iter().copied().collect())
                    .unwrap_or_default();
                if b.is_none() {
                    unknown.extend(
                        dependencies
                            .iter()
                            .filter(|d| !d.removed && d.state == BlockerState::Unknown)
                            .map(|d| format!("#{} ← #{}", i.number, d.blocker)),
                    );
                }
                let skip = b.or_else(|| {
                    if cycle.contains(&i.number) {
                        Some("dependency_cycle")
                    } else if !blocked_by.is_empty() {
                        Some("blocked_by")
                    } else if unanalysed.iter().any(|u| u.number == i.number) {
                        Some("analysis_pending")
                    } else {
                        None
                    }
                });
                QueueItem {
                    number: i.number,
                    title: i.title.clone(),
                    priority: i.priority,
                    model: i.model.clone(),
                    skip: skip.map(String::from),
                    blocked_by,
                    dependencies,
                    order: p.order.get(&i.number).copied(),
                }
            })
            .collect();
        let notice = |kind: &'static str, scope: String, reason: &str, message: String| {
            (
                kind,
                Refusal {
                    scope,
                    reason: reason.into(),
                    message,
                    account: account.clone(),
                    env_id: Some(p.env_id.clone()),
                    owner: Some(p.owner.clone()),
                    repo: Some(p.repo.clone()),
                },
            )
        };
        let in_cycle: Vec<String> = p
            .items
            .iter()
            .zip(&base)
            .filter(|(i, b)| b.is_none() && cycle.contains(&i.number))
            .map(|(i, _)| format!("#{}", i.number))
            .collect();
        if !in_cycle.is_empty() {
            out.notices.push(notice(
                EV_DEPENDENCY_CYCLE,
                format!("cycle:{slug}"),
                EV_DEPENDENCY_CYCLE,
                format!(
                    "{slug}: ciclo de dependências entre {}; essas issues ficam fora da fila até \
                     alguém corrigir uma aresta",
                    cycle
                        .iter()
                        .map(|n| format!("#{n}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
        if !unknown.is_empty() {
            out.notices.push(notice(
                EV_DEPENDENCY_UNKNOWN,
                format!("deps:{slug}"),
                EV_DEPENDENCY_UNKNOWN,
                format!(
                    "{slug}: bloqueante fora do espelho do projeto ({}); a issue segue bloqueada \
                     até alguém remover a aresta",
                    unknown.join(", ")
                ),
            ));
        }
        if gate == Some(false) && !candidates.is_empty() {
            let name = e.map(|e| e.name.clone()).unwrap_or_default();
            out.notices.push(notice(
                EV_ANALYSIS_UNSUPPORTED,
                format!("analysis:{}", p.env_id),
                EV_ANALYSIS_UNSUPPORTED,
                format!(
                    "o ambiente autônomo {name} é anterior ao protocolo {}.{}: sem a análise de \
                     dependências; só as arestas já gravadas valem (atualize o ambiente)",
                    ANALYSIS_PROTOCOL.0, ANALYSIS_PROTOCOL.1
                ),
            ));
        }
        let round = rounds.get(&p.project_id).cloned();
        let analysis_view = round.as_ref().map(|r| AnalysisView {
            state: if r.session_id.is_none() {
                "failed"
            } else if r.planned_at.is_none() {
                "planning"
            } else if !r.pending.is_empty() {
                "reanalysing"
            } else {
                "ready"
            }
            .into(),
            session_id: r.session_id.clone(),
            opened_at: r.opened_at,
            planned_at: r.planned_at,
            pending: r.pending.keys().copied().collect(),
            failed_at: r.failed_at,
        });
        let mut analysis_action: Option<Action> = None;
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
            analysis: analysis_view,
            analysis_unsupported: gate == Some(false),
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
            // Análise de dependências (#215): não ocupa a vaga do ambiente,
            // mas vem antes de qualquer implementação no projeto.
            if !candidates.is_empty() {
                match gate {
                    None => {
                        return Some((
                            "analysis".into(),
                            format!(
                                "{slug}: aguardando o ambiente autônomo informar a versão (análise \
                                 de dependências)"
                            ),
                            None,
                        ));
                    }
                    Some(false) => {}
                    Some(true) => {
                        let planned = round.as_ref().is_some_and(|r| r.planned_at.is_some());
                        match round
                            .as_ref()
                            .and_then(|r| r.session_id.clone().map(|sid| (r, sid)))
                        {
                            None => {
                                let retry_at = round
                                    .as_ref()
                                    .and_then(|r| r.failed_at)
                                    .map(|f| f + ANALYSIS_RETRY_MS);
                                if retry_at.is_some_and(|t| now < t) {
                                    if !planned {
                                        return Some((
                                            "analysis".into(),
                                            format!(
                                                "{slug}: a sessão de análise falhou; outra inicia \
                                                 depois de {} min",
                                                ANALYSIS_RETRY_MS / 60_000
                                            ),
                                            None,
                                        ));
                                    }
                                } else if started_accounts.contains(key) {
                                    return Some((
                                        "account_turn".into(),
                                        "a conta já iniciou uma sessão neste ciclo".into(),
                                        None,
                                    ));
                                } else {
                                    let pending = candidates
                                        .iter()
                                        .map(|i| (i.number, i.fingerprint.clone()))
                                        .collect();
                                    let r = match round.clone() {
                                        Some(r) => AnalysisRound {
                                            pending,
                                            started_at: Some(now),
                                            sent_at: Some(now),
                                            env_id: p.env_id.clone(),
                                            ..r
                                        },
                                        None => AnalysisRound {
                                            project_id: p.project_id,
                                            session_id: None,
                                            env_id: p.env_id.clone(),
                                            opened_at: now,
                                            started_at: Some(now),
                                            planned_at: None,
                                            pending,
                                            sent_at: Some(now),
                                            failed_at: None,
                                        },
                                    };
                                    started_accounts.insert(key.clone());
                                    analysis_action = Some(Action::StartAnalysis(AnalysisStart {
                                        account: key.clone(),
                                        env_id: p.env_id.clone(),
                                        owner: p.owner.clone(),
                                        repo: p.repo.clone(),
                                        prompt: deps::analysis_prompt(p, &candidates),
                                        reason: if planned {
                                            ANALYSIS_REANALYSIS
                                        } else {
                                            ANALYSIS_PLANNING
                                        },
                                        round: r,
                                    }));
                                    return Some((
                                        "analysis".into(),
                                        format!(
                                            "{slug}: a sessão de análise de dependências iniciou; \
                                             as implementações esperam o planejamento"
                                        ),
                                        None,
                                    ));
                                }
                            }
                            Some((r, sid)) => {
                                let idle = s.analysis_states.get(&sid) == Some(&SessionState::Idle);
                                if planned && r.pending.is_empty() && !unanalysed.is_empty() && idle
                                {
                                    let send: Vec<&ItemSnap> = unanalysed
                                        .iter()
                                        .copied()
                                        .take(deps::REANALYSIS_MAX_ITEMS)
                                        .collect();
                                    let nums = send
                                        .iter()
                                        .map(|i| format!("#{}", i.number))
                                        .collect::<Vec<_>>()
                                        .join(", ");
                                    analysis_action = Some(Action::ContinueAnalysis {
                                        round: AnalysisRound {
                                            pending: send
                                                .iter()
                                                .map(|i| (i.number, i.fingerprint.clone()))
                                                .collect(),
                                            sent_at: Some(now),
                                            ..r.clone()
                                        },
                                        owner: p.owner.clone(),
                                        repo: p.repo.clone(),
                                        text: deps::reanalysis_text(&send),
                                        message: format!(
                                            "{slug}: issues novas ou alteradas na fila ({nums}); \
                                             a sessão de análise foi acordada para reanalisar"
                                        ),
                                    });
                                }
                            }
                        }
                        if !planned {
                            return Some((
                                "analysis".into(),
                                format!(
                                    "{slug}: o planejamento de dependências da janela está em \
                                     andamento; nenhuma implementação começa antes dele"
                                ),
                                None,
                            ));
                        }
                    }
                }
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
        out.actions.extend(analysis_action);
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
                            title: i.title.clone().expect("elegível está no espelho"),
                            model: i.model.clone().expect("elegível tem modelo"),
                            probe,
                        }));
                    }
                    None => {
                        let m = format!(
                            "{slug}: nenhuma issue elegível (aprovada, com prioridade, com \
                             modelo, sem sessão viva e sem bloqueante pendente)"
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
            fingerprint: format!("fp-{n}"),
            epic_run: false,
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
            integration: "wip/autonoma".into(),
            ..Default::default()
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
                continue_text: Some(true),
                // Os testes anteriores à #215 rodam sem a análise.
                analysis: Some(false),
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

    /// #303: a sub-issue aprovada e com prioridade de um épico em execução
    /// fica fora da fila; com a execução encerrada, volta.
    #[test]
    fn sub_issue_de_epico_em_execucao_fica_fora() {
        let mut s = snap(SAB);
        s.projects[0].items[0].epic_run = true;
        let o = decide(&s);
        assert!(starts(&o).is_empty());
        let q = &o.projects[0].queue[0];
        assert_eq!((q.number, q.skip.as_deref()), (123, Some("epic_run")));
        s.projects[0].items[0].epic_run = false;
        assert_eq!(
            starts(&decide(&s)),
            vec![("acme-web".into(), 123, "opus".into())]
        );
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
                continue_text: Some(true),
                analysis: Some(false),
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
                continue_text: Some(true),
                analysis: Some(false),
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

    fn idle(s: &mut Snapshot, id: &str, since: i64, autonomous: bool, done: bool) {
        s.idle.insert(
            id.into(),
            IdleSession {
                session_id: id.into(),
                since,
                autonomous,
                done,
                work_item_id: 1,
            },
        );
    }

    fn ends(o: &Outcome) -> Vec<(String, &'static str)> {
        o.actions
            .iter()
            .filter_map(|a| match a {
                Action::End {
                    session, reason, ..
                } => Some((session.session_id.clone(), *reason)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn sessao_autonoma_ociosa_com_pr_encerra_depois_da_folga() {
        let mut s = snap(SAB);
        s.sessions
            .push(session("s1", "env-auto", 123, SessionState::Idle));
        idle(&mut s, "s1", SAB - DONE_GRACE_MS + 1, true, true);
        // Dentro da folga: nada; o ambiente segue ocupado neste ciclo.
        let o = decide(&s);
        assert!(ends(&o).is_empty());
        assert_eq!(o.projects[0].blocked.as_deref(), Some("env_busy"));
        idle(&mut s, "s1", SAB - DONE_GRACE_MS, true, true);
        let o = decide(&s);
        assert_eq!(ends(&o), vec![("s1".to_string(), END_DONE)]);
        match &o.actions[0] {
            Action::End { message, .. } => assert!(message.contains("acme/acme-web#123")),
            a => panic!("{a:?}"),
        }
        // A vaga só abre quando o ambiente sincroniza o encerramento.
        assert!(starts(&o).is_empty());
        // Já encerrada (o ambiente não sincronizou): não encerra de novo.
        s.ended.insert("s1".into());
        assert!(ends(&decide(&s)).is_empty());
        s.ended.clear();
        // Ambiente offline: tenta no ciclo em que voltar.
        s.envs.get_mut("env-auto").unwrap().online = false;
        assert!(ends(&decide(&s)).is_empty());
    }

    #[test]
    fn sessao_autonoma_ociosa_sem_pr_encerra_depois_do_limite() {
        let mut s = snap(SAB);
        s.settings.idle_timeout_min = 30;
        s.sessions
            .push(session("s1", "env-auto", 123, SessionState::Idle));
        idle(&mut s, "s1", SAB - 29 * 60_000, true, false);
        assert!(ends(&decide(&s)).is_empty());
        idle(&mut s, "s1", SAB - 30 * 60_000, true, false);
        let o = decide(&s);
        assert_eq!(ends(&o), vec![("s1".to_string(), END_IDLE)]);
        match &o.actions[0] {
            Action::End { message, .. } => assert!(message.contains("30 min"), "{message}"),
            a => panic!("{a:?}"),
        }
        // Pausado também: a pausa só segura os inícios.
        s.settings.paused = true;
        assert_eq!(ends(&decide(&s)).len(), 1);
        s.settings.paused = false;
        // 0 desliga o limite sem PR.
        s.settings.idle_timeout_min = 0;
        assert!(ends(&decide(&s)).is_empty());
    }

    #[test]
    fn so_encerra_sessao_autonoma_ociosa_de_ambiente_autonomo() {
        let mut s = snap(SAB);
        s.settings.idle_timeout_min = 30;
        s.sessions
            .push(session("s1", "env-auto", 123, SessionState::Idle));
        // Sessão comum (sem início autônomo): nunca.
        idle(&mut s, "s1", SAB - 10 * H, false, true);
        assert!(ends(&decide(&s)).is_empty());
        // Rodando: nunca (só a ociosa).
        idle(&mut s, "s1", SAB - 10 * H, true, true);
        s.sessions[0].state = SessionState::Running;
        assert!(ends(&decide(&s)).is_empty());
        s.sessions[0].state = SessionState::Idle;
        // Ambiente que não é autônomo: nunca.
        s.envs.get_mut("env-auto").unwrap().autonomous = false;
        assert!(ends(&decide(&s)).is_empty());
    }

    #[test]
    fn estacionada_nunca_encerra_e_a_que_terminou_nao_estaciona() {
        // Conta em 100% da 5h: a ociosa sem PR estaciona, não encerra.
        let mut s = snap(SAB);
        s.settings.idle_timeout_min = 30;
        s.accounts
            .insert("conta-a".into(), account(100.0, 40.0, SAB));
        s.sessions
            .push(session("s1", "env-auto", 123, SessionState::Idle));
        idle(&mut s, "s1", SAB - 10 * H, true, false);
        let o = decide(&s);
        assert!(ends(&o).is_empty());
        let parked = match &o.actions[..] {
            [Action::Park(p)] => p.clone(),
            a => panic!("{a:?}"),
        };
        // Estacionada, esperando o reset: nem com a conta já renovada, nem
        // com a issue com PR, encerra.
        s.parked.push(parked);
        s.accounts
            .insert("conta-a".into(), account(10.0, 40.0, SAB));
        assert!(ends(&decide(&s)).is_empty());
        idle(&mut s, "s1", SAB - 10 * H, true, true);
        assert!(ends(&decide(&s)).is_empty());
        // A que terminou a issue (PR aberto) não estaciona: encerra.
        s.parked.clear();
        s.accounts
            .insert("conta-a".into(), account(100.0, 40.0, SAB));
        let o = decide(&s);
        assert_eq!(ends(&o), vec![("s1".to_string(), END_DONE)]);
        assert!(!o.actions.iter().any(|a| matches!(a, Action::Park(_))));
    }
    // --- Fechamento autônomo (#184) ---

    use ct_core::PrState;
    use ct_ledger::{ChangesRequest, CheckSource, PrCheck, ReviewThread};

    fn check(name: &str, conclusion: Option<&str>) -> PrCheck {
        PrCheck {
            name: name.into(),
            source: CheckSource::CheckRun,
            conclusion: conclusion.map(String::from),
            url: None,
            job_id: Some(1),
        }
    }

    fn pr_row(head_sha: &str) -> PrRow {
        PrRow {
            number: 9,
            work_item_id: 1,
            head: "feat/123-login".into(),
            base: "wip/autonoma".into(),
            state: PrState::Open,
            url: "https://github.com/acme/acme-web/pull/9".into(),
            updated_at: 50,
            head_sha: Some(head_sha.into()),
        }
    }

    fn checks_of(head_sha: &str, checks: Vec<PrCheck>) -> PrChecks {
        PrChecks {
            pr_number: 9,
            head: "feat/123-login".into(),
            base: "wip/autonoma".into(),
            head_sha: head_sha.into(),
            state: ct_ledger::checks_state(&checks),
            checks,
            synced_at: 1,
            runs_etag: None,
            status_etag: None,
        }
    }

    fn feedback(threads: Vec<ReviewThread>, changes: Vec<ChangesRequest>) -> PrFeedback {
        PrFeedback {
            pr: pr_row("h1"),
            read_at_updated: Some(50),
            threads,
            changes_requested: changes,
        }
    }

    fn thread(id: &str, external: bool) -> ReviewThread {
        ReviewThread {
            id: id.into(),
            path: "src/login.rs".into(),
            line: Some(3),
            outdated: false,
            author: "fulano".into(),
            author_association: if external { "NONE" } else { "MEMBER" }.into(),
            external,
            body: "trocar o nome".into(),
            url: "https://github.com/acme/acme-web/pull/9#r1".into(),
            replies: 0,
            created_at: 1,
        }
    }

    /// Sessão `s1` da issue 123 ociosa (desde `SAB - 2 min`), com o PR #9
    /// contra a `wip/autonoma` no head `head` e esses checks.
    fn closing_snap(head: &str, checks: Option<PrChecks>) -> Snapshot {
        let mut s = snap(SAB);
        s.sessions
            .push(session("s1", "env-auto", 123, SessionState::Idle));
        idle(&mut s, "s1", SAB - 2 * 60_000, true, true);
        s.closing.insert(
            "s1".into(),
            Closing {
                work_item_id: 1,
                integration: "wip/autonoma".into(),
                pr: pr_row(head),
                checks,
                feedback: Some(feedback(vec![], vec![])),
                state: CloseState::default(),
            },
        );
        s
    }

    fn sends(o: &Outcome) -> Vec<(SendKind, CloseState, String)> {
        o.actions
            .iter()
            .filter_map(|a| match a {
                Action::Send {
                    kind, close, text, ..
                } => Some((*kind, close.clone(), text.clone())),
                _ => None,
            })
            .collect()
    }

    fn blocks(o: &Outcome) -> Vec<(&'static str, String)> {
        o.actions
            .iter()
            .filter_map(|a| match a {
                Action::Block {
                    reason,
                    status_reason,
                    ..
                } => Some((*reason, status_reason.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn ci_pendente_ou_nao_lido_espera_e_nao_encerra() {
        // Checks ainda não lidos.
        let o = decide(&closing_snap("h1", None));
        assert!(
            ends(&o).is_empty() && sends(&o).is_empty(),
            "{:?}",
            o.actions
        );
        // Checks de um head anterior: ainda não lidos para o atual.
        let o = decide(&closing_snap(
            "h2",
            Some(checks_of("h1", vec![check("test", Some("success"))])),
        ));
        assert!(o.actions.is_empty(), "{:?}", o.actions);
        // Em andamento (mesmo com um que já falhou): espera, só grava o head.
        let mut s = closing_snap(
            "h1",
            Some(checks_of(
                "h1",
                vec![check("test", Some("failure")), check("lint", None)],
            )),
        );
        // Muito tempo depois, ainda espera.
        s.settings.idle_timeout_min = 30;
        s.idle.get_mut("s1").unwrap().since = SAB - 10 * H;
        let o = decide(&s);
        assert!(ends(&o).is_empty() && sends(&o).is_empty() && blocks(&o).is_empty());
        match &o.actions[..] {
            [Action::Track(c)] => {
                assert_eq!(c.head_sha.as_deref(), Some("h1"));
                assert_eq!(c.head_seen_at, Some(SAB));
            }
            a => panic!("{a:?}"),
        }
        // Sem o `pr_aberto` antigo: o ambiente segue ocupado.
        assert_eq!(o.projects[0].blocked.as_deref(), Some("env_busy"));
    }

    #[test]
    fn falha_volta_ao_agente_uma_vez_por_head_e_conta_tentativa() {
        let mut s = closing_snap(
            "h1",
            Some(checks_of("h1", vec![check("test", Some("failure"))])),
        );
        let o = decide(&s);
        let sent = sends(&o);
        assert_eq!(sent.len(), 1, "{:?}", o.actions);
        let (kind, close, text) = &sent[0];
        assert_eq!(*kind, SendKind::Feedback(FEEDBACK_CI));
        assert_eq!(close.attempts, 1);
        assert_eq!(close.feedback_head.as_deref(), Some("h1"));
        assert_eq!(close.last_sent_at, Some(SAB));
        assert!(text.starts_with(FEEDBACK_REQUEST), "{text}");
        assert!(text.contains("pr_check_log"), "{text}");
        assert!(text.contains("Falhou: `test`"), "{text}");
        assert!(ends(&o).is_empty());
        // Já enviado para o h1: nada de novo (a sessão ainda não rodou).
        s.closing.get_mut("s1").unwrap().state = close.clone();
        let o = decide(&s);
        assert!(o.actions.is_empty(), "{:?}", o.actions);
        // Corrigido: head novo, verde. Pede o merge.
        let c = s.closing.get_mut("s1").unwrap();
        c.pr = pr_row("h2");
        c.checks = Some(checks_of("h2", vec![check("test", Some("success"))]));
        let o = decide(&s);
        let sent = sends(&o);
        assert_eq!(sent.len(), 1, "{:?}", o.actions);
        let (kind, close, text) = &sent[0];
        assert_eq!(*kind, SendKind::Merge);
        assert_eq!(close.merge_head.as_deref(), Some("h2"));
        assert_eq!(close.attempts, 1);
        assert!(
            text.contains("`gh pr merge 9 --rebase`") && text.contains("wip/autonoma"),
            "{text}"
        );
        // Uma vez por head.
        s.closing.get_mut("s1").unwrap().state = close.clone();
        assert!(sends(&decide(&s)).is_empty());
        // Ainda parada muito depois do pedido: bloqueia sem progresso.
        s.settings.idle_timeout_min = 30;
        s.now = SAB + 31 * 60_000;
        let o = decide(&s);
        let b = blocks(&o);
        assert_eq!(b.len(), 1, "{:?}", o.actions);
        assert_eq!(b[0].0, BLOCK_STALLED);
        assert!(b[0].1.contains("merge foi pedido"), "{}", b[0].1);
    }

    #[test]
    fn tentativas_esgotadas_bloqueiam_a_issue() {
        let mut s = closing_snap(
            "h4",
            Some(checks_of("h4", vec![check("test", Some("failure"))])),
        );
        s.closing.get_mut("s1").unwrap().state = CloseState {
            work_item_id: 1,
            attempts: 3,
            feedback_head: Some("h3".into()),
            ..Default::default()
        };
        let o = decide(&s);
        let b = blocks(&o);
        assert_eq!(b.len(), 1, "{:?}", o.actions);
        assert_eq!(b[0].0, BLOCK_ATTEMPTS);
        assert!(b[0].1.contains("3 tentativas"), "{}", b[0].1);
        assert!(sends(&o).is_empty() && ends(&o).is_empty());
        // Ainda com tentativa sobrando: envia a quarta.
        s.settings.max_ci_attempts = 4;
        assert_eq!(sends(&decide(&s))[0].1.attempts, 4);
        // 0 = bloqueia na primeira falha.
        s.settings.max_ci_attempts = 0;
        s.closing.get_mut("s1").unwrap().state = CloseState::default();
        let b = blocks(&decide(&s));
        assert_eq!(b[0].0, BLOCK_ATTEMPTS);
        assert!(b[0].1.contains("sem reenvio"), "{}", b[0].1);
    }

    #[test]
    fn review_de_colaborador_volta_ao_agente_e_o_externo_nao_segura_o_merge() {
        let mut s = closing_snap(
            "h1",
            Some(checks_of("h1", vec![check("test", Some("success"))])),
        );
        s.closing.get_mut("s1").unwrap().feedback = Some(feedback(
            vec![thread("t-ext", true), thread("t-in", false)],
            vec![],
        ));
        let o = decide(&s);
        let sent = sends(&o);
        assert_eq!(sent.len(), 1, "{:?}", o.actions);
        assert_eq!(sent[0].0, SendKind::Feedback(FEEDBACK_REVIEW));
        assert_eq!(sent[0].1.threads_sent, vec!["t-in".to_string()]);
        assert_eq!(sent[0].1.attempts, 0);
        assert!(sent[0].2.contains("texto omitido"), "{}", sent[0].2);
        // Enviada e ainda não resolvida: não pede o merge.
        s.closing.get_mut("s1").unwrap().state = sent[0].1.clone();
        assert!(sends(&decide(&s)).is_empty());
        // Só a externa: o merge sai.
        s.closing.get_mut("s1").unwrap().feedback =
            Some(feedback(vec![thread("t-ext", true)], vec![]));
        assert_eq!(sends(&decide(&s))[0].0, SendKind::Merge);
        // Review que pede mudanças, de colaborador: volta ao agente.
        s.closing.get_mut("s1").unwrap().feedback = Some(feedback(
            vec![],
            vec![ChangesRequest {
                id: "r1".into(),
                author: "fulano".into(),
                author_association: "COLLABORATOR".into(),
                external: false,
                body: "faltou teste".into(),
                url: "https://github.com/acme/acme-web/pull/9#r".into(),
                submitted_at: 1,
            }],
        ));
        assert_eq!(sends(&decide(&s))[0].0, SendKind::Feedback(FEEDBACK_REVIEW));
        // Reviews ainda não lidas para a versão atual do PR: espera o merge.
        let c = s.closing.get_mut("s1").unwrap();
        c.feedback = Some(PrFeedback {
            read_at_updated: Some(10),
            ..feedback(vec![], vec![])
        });
        assert!(sends(&decide(&s)).is_empty());
    }

    #[test]
    fn repositorio_sem_checks_conta_como_verde_depois_do_prazo() {
        let mut s = closing_snap("h1", Some(checks_of("h1", vec![])));
        let o = decide(&s);
        assert!(sends(&o).is_empty());
        let seen = match &o.actions[..] {
            [Action::Track(c)] => c.clone(),
            a => panic!("{a:?}"),
        };
        s.closing.get_mut("s1").unwrap().state = seen;
        s.now = SAB + NO_CHECKS_GREEN_MS - 1;
        assert!(sends(&decide(&s)).is_empty());
        s.now = SAB + NO_CHECKS_GREEN_MS;
        assert_eq!(sends(&decide(&s))[0].0, SendKind::Merge);
    }

    #[test]
    fn ambiente_antigo_cai_no_pr_aberto_e_avisa() {
        let mut s = closing_snap(
            "h1",
            Some(checks_of("h1", vec![check("test", Some("failure"))])),
        );
        idle(&mut s, "s1", SAB - DONE_GRACE_MS, true, true);
        s.envs.get_mut("env-auto").unwrap().continue_text = Some(false);
        let o = decide(&s);
        assert_eq!(ends(&o), vec![("s1".to_string(), END_DONE)]);
        assert!(sends(&o).is_empty());
        let r: Vec<_> = o
            .refusals
            .iter()
            .filter(|r| r.scope == "env:env-auto")
            .collect();
        assert_eq!(r.len(), 1, "{:?}", o.refusals);
        assert_eq!(r[0].reason, ENV_OUTDATED);
        assert!(r[0].message.contains("2.22"), "{}", r[0].message);
        // Versão desconhecida (o `env.info` não respondeu): espera.
        s.envs.get_mut("env-auto").unwrap().continue_text = None;
        assert!(decide(&s).actions.is_empty());
        // Sem PR na branch de integração (outra base, ou já mesclado): o
        // `pr_aberto` de antes, sem pedir merge.
        s.closing.clear();
        s.envs.get_mut("env-auto").unwrap().continue_text = Some(true);
        let o = decide(&s);
        assert_eq!(ends(&o), vec![("s1".to_string(), END_DONE)]);
        assert!(sends(&o).is_empty());
    }

    #[test]
    fn conta_esgotada_estaciona_em_vez_de_fechar() {
        let mut s = closing_snap(
            "h1",
            Some(checks_of("h1", vec![check("test", Some("failure"))])),
        );
        s.accounts
            .insert("conta-a".into(), account(100.0, 40.0, SAB));
        let o = decide(&s);
        assert!(sends(&o).is_empty() && ends(&o).is_empty());
        assert!(
            matches!(o.actions[..], [Action::Park(_)]),
            "{:?}",
            o.actions
        );
    }

    fn waiting(s: &mut Snapshot, id: &str, since: i64, autonomous: bool) {
        s.waiting.insert(
            id.into(),
            WaitingSession {
                session_id: id.into(),
                since,
                autonomous,
                work_item_id: 1,
            },
        );
    }

    #[test]
    fn aguardando_voce_alem_do_prazo_bloqueia() {
        let mut s = snap(SAB);
        s.sessions
            .push(session("s1", "env-auto", 123, SessionState::Running));
        waiting(&mut s, "s1", SAB - 19 * 60_000, true);
        assert!(blocks(&decide(&s)).is_empty());
        waiting(&mut s, "s1", SAB - 20 * 60_000, true);
        let o = decide(&s);
        let b = blocks(&o);
        assert_eq!(b.len(), 1, "{:?}", o.actions);
        assert_eq!(b[0].0, BLOCK_WAITING);
        assert!(
            b[0].1.contains("aguardando você há mais de 20 min"),
            "{}",
            b[0].1
        );
        // Ociosa também (e sem o `sessao_ociosa` junto).
        s.sessions[0].state = SessionState::Idle;
        s.settings.idle_timeout_min = 5;
        idle(&mut s, "s1", SAB - H, true, false);
        let o = decide(&s);
        assert_eq!(blocks(&o).len(), 1);
        assert!(ends(&o).is_empty());
        // 0 desliga.
        s.settings.waiting_timeout_min = 0;
        assert!(blocks(&decide(&s)).is_empty());
        s.settings.waiting_timeout_min = 20;
        // Sessão comum: nunca.
        waiting(&mut s, "s1", SAB - H, false);
        assert!(blocks(&decide(&s)).is_empty());
        waiting(&mut s, "s1", SAB - H, true);
        // Estacionada pelo limite do plano: nunca.
        s.parked.push(ParkedSession {
            session_id: "s1".into(),
            env_id: "env-auto".into(),
            account_key: "conta-a".into(),
            owner: "acme".into(),
            repo: "acme-web".into(),
            gh_number: 123,
            window: "five_hour".into(),
            resume_at: SAB + H,
            parked_at: SAB - H,
        });
        assert!(blocks(&decide(&s)).is_empty());
        s.parked.clear();
        // Conta com a janela esgotada: estaciona, não bloqueia.
        s.accounts
            .insert("conta-a".into(), account(100.0, 40.0, SAB));
        let o = decide(&s);
        assert!(blocks(&o).is_empty());
        assert!(
            matches!(o.actions[..], [Action::Park(_)]),
            "{:?}",
            o.actions
        );
    }

    #[test]
    fn protocolo_do_ambiente() {
        assert_eq!(parse_protocol("2.22"), Some((2, 22)));
        assert_eq!(parse_protocol(" 2.3.1"), Some((2, 3)));
        assert_eq!(parse_protocol("x"), None);
        assert!(parse_protocol("2.21").unwrap() < CONTINUE_TEXT_PROTOCOL);
        assert!(parse_protocol("3.0").unwrap() >= CONTINUE_TEXT_PROTOCOL);
    }

    #[test]
    fn feedback_grande_demais_vai_sem_os_dados() {
        let pr = pr_row("h1");
        let big: Vec<ReviewThread> = (0..40)
            .map(|i| ReviewThread {
                body: "x".repeat(2000),
                ..thread(&format!("t{i}"), false)
            })
            .collect();
        let t = feedback_text(&pr, None, Some(&feedback(big, vec![])));
        assert!(t.len() <= ct_core::autonomy::CONTINUE_TEXT_MAX_BYTES);
        assert!(
            t.starts_with(FEEDBACK_REQUEST) && t.contains("pr_feedback"),
            "{t}"
        );
        assert!(!t.contains(ct_ledger::DATA_OPEN));
    }

    // --- Análise de dependências (#215) ---------------------------------

    /// Terça 18:01 em GMT-3: a "Noite de dia útil" acabou de abrir.
    const TER_18: i64 = TER_CORTE + 13 * H + 60_000;
    const MIN: i64 = 60_000;

    fn dep(n: i64, b: i64, source: &str, removed: bool) -> DependencyEdge {
        DependencyEdge {
            number: n,
            blocker: b,
            reason: format!("precisa da #{b}"),
            source: source.into(),
            removed,
            updated_at: 1,
        }
    }

    /// `snap` com a análise ligada no ambiente: o projeto `acme-web` (id 1)
    /// com a 123 elegível (P1, opus) e a 124 sem modelo.
    fn snap_analise(now: i64) -> Snapshot {
        let mut s = snap(now);
        s.envs.get_mut("env-auto").unwrap().analysis = Some(true);
        s.projects[0].project_id = 1;
        s
    }

    /// A 123 (e as demais elegíveis) já analisadas na versão atual.
    fn analisadas(s: &mut Snapshot) {
        let p = &mut s.projects[0];
        for i in &p.items {
            p.analysed.insert(i.number, i.fingerprint.clone());
        }
    }

    fn rodada(
        s: &mut Snapshot,
        session: Option<(&str, SessionState)>,
        planned_at: Option<i64>,
        at: i64,
    ) {
        if let Some((id, st)) = session {
            s.analysis_states.insert(id.into(), st);
        }
        s.analyses.insert(
            1,
            AnalysisSnap {
                owner: "acme".into(),
                repo: "acme-web".into(),
                round: AnalysisRound {
                    project_id: 1,
                    session_id: session.map(|(id, _)| id.to_string()),
                    env_id: "env-auto".into(),
                    opened_at: at,
                    started_at: Some(at),
                    planned_at,
                    sent_at: Some(at),
                    ..Default::default()
                },
            },
        );
    }

    fn analysis_starts(o: &Outcome) -> Vec<&AnalysisStart> {
        o.actions
            .iter()
            .filter_map(|a| match a {
                Action::StartAnalysis(x) => Some(x),
                _ => None,
            })
            .collect()
    }

    fn closes(o: &Outcome) -> usize {
        o.actions
            .iter()
            .filter(|a| matches!(a, Action::CloseAnalysis { .. }))
            .count()
    }

    fn skip_of_item(o: &Outcome, n: i64) -> Option<String> {
        o.projects[0]
            .queue
            .iter()
            .find(|q| q.number == n)
            .unwrap()
            .skip
            .clone()
    }

    #[test]
    fn janela_abre_analisa_antes_de_implementar_e_fecha_no_fim() {
        // Terça 17:00: horário comercial, janela fechada: nada de análise.
        let antes = TER_18 - H;
        let o = decide(&snap_analise(antes));
        assert!(analysis_starts(&o).is_empty());
        assert!(starts(&o).is_empty());
        assert_eq!(o.projects[0].blocked.as_deref(), Some("budget:band_closed"));

        // 18:01: a janela abre e a análise inicia antes de qualquer
        // implementação.
        let o = decide(&snap_analise(TER_18));
        let a = analysis_starts(&o);
        assert_eq!(a.len(), 1, "{:?}", o.actions);
        assert!(starts(&o).is_empty());
        assert_eq!(a[0].reason, ANALYSIS_PLANNING);
        assert_eq!(a[0].env_id, "env-auto");
        assert_eq!(
            a[0].round.pending,
            BTreeMap::from([(123, "fp-123".to_string())])
        );
        assert_eq!(a[0].round.opened_at, TER_18);
        assert!(
            a[0].prompt.contains("- #123 ACME-123 (P1)"),
            "{}",
            a[0].prompt
        );
        assert!(!a[0].prompt.contains("#124"), "{}", a[0].prompt);
        assert_eq!(o.projects[0].blocked.as_deref(), Some("analysis"));

        // Planejamento em andamento: nenhuma implementação, nenhuma outra
        // análise.
        let mut s = snap_analise(TER_18 + 5 * MIN);
        rodada(
            &mut s,
            Some(("s-analise", SessionState::Running)),
            None,
            TER_18,
        );
        let o = decide(&s);
        assert!(analysis_starts(&o).is_empty());
        assert!(starts(&o).is_empty());
        assert_eq!(o.projects[0].blocked.as_deref(), Some("analysis"));
        assert_eq!(o.projects[0].analysis.as_ref().unwrap().state, "planning");

        // Planejamento gravado: a 123 inicia. A sessão de análise (#0) não
        // ocupa a vaga do ambiente (uma sessão por ambiente).
        let mut s = snap_analise(TER_18 + 30 * MIN);
        rodada(
            &mut s,
            Some(("s-analise", SessionState::Idle)),
            Some(TER_18 + 20 * MIN),
            TER_18,
        );
        analisadas(&mut s);
        assert_eq!(s.settings.max_sessions_per_env, 1);
        let o = decide(&s);
        assert_eq!(starts(&o), vec![("acme-web".into(), 123, "opus".into())]);
        assert_eq!(o.projects[0].analysis.as_ref().unwrap().state, "ready");

        // Quarta 08:01: a janela fecha; a análise é encerrada e a
        // implementação em andamento continua.
        let fim = TER_CORTE + 27 * H + MIN;
        let mut s = snap_analise(fim);
        rodada(
            &mut s,
            Some(("s-analise", SessionState::Idle)),
            Some(TER_18 + 20 * MIN),
            TER_18,
        );
        analisadas(&mut s);
        s.sessions
            .push(session("s-123", "env-auto", 123, SessionState::Running));
        let o = decide(&s);
        assert_eq!(closes(&o), 1, "{:?}", o.actions);
        assert!(!o
            .actions
            .iter()
            .any(|a| matches!(a, Action::End { .. } | Action::Block { .. })));
        assert!(analysis_starts(&o).is_empty());
        assert!(o.projects[0].analysis.is_none());

        // Fora da janela, sem rodada: nada.
        let o = decide(&snap_analise(fim + H));
        assert!(analysis_starts(&o).is_empty());
        assert_eq!(closes(&o), 0);
    }

    #[test]
    fn issue_nova_ou_alterada_acorda_a_analise_e_espera_a_reanalise() {
        let mut s = snap_analise(SAB);
        rodada(
            &mut s,
            Some(("s-analise", SessionState::Idle)),
            Some(SAB - H),
            SAB - 2 * H,
        );
        analisadas(&mut s);
        s.projects[0]
            .items
            .insert(0, item(125, Some(Priority::P0), Some("sonnet"), true));
        let o = decide(&s);
        // A 125 (nova, P0) espera; a 123, já analisada, segue.
        assert_eq!(skip_of_item(&o, 125).as_deref(), Some("analysis_pending"));
        assert_eq!(starts(&o), vec![("acme-web".into(), 123, "opus".into())]);
        let sent: Vec<_> = o
            .actions
            .iter()
            .filter_map(|a| match a {
                Action::ContinueAnalysis { round, text, .. } => Some((round, text)),
                _ => None,
            })
            .collect();
        assert_eq!(sent.len(), 1, "{:?}", o.actions);
        assert_eq!(
            sent[0].0.pending,
            BTreeMap::from([(125, "fp-125".to_string())])
        );
        assert!(sent[0].1.contains("#125"));
        assert!(!sent[0].1.contains("#123"));

        // Já enviada: não envia de novo enquanto a reanálise não volta.
        {
            let r = &mut s.analyses.get_mut(&1).unwrap().round;
            r.pending = BTreeMap::from([(125, "fp-125".to_string())]);
            r.sent_at = Some(SAB - MIN);
        }
        let o = decide(&s);
        assert!(!o
            .actions
            .iter()
            .any(|a| matches!(a, Action::ContinueAnalysis { .. })));
        assert_eq!(skip_of_item(&o, 125).as_deref(), Some("analysis_pending"));
        assert_eq!(
            o.projects[0].analysis.as_ref().unwrap().state,
            "reanalysing"
        );

        // A sessão rodando (ainda respondendo) não é acordada.
        s.analyses.get_mut(&1).unwrap().round.pending.clear();
        s.analysis_states
            .insert("s-analise".into(), SessionState::Running);
        let o = decide(&s);
        assert!(!o
            .actions
            .iter()
            .any(|a| matches!(a, Action::ContinueAnalysis { .. })));

        // Reanalisada: a 125 entra na fila (P0 primeiro).
        s.projects[0].analysed.insert(125, "fp-125".into());
        let o = decide(&s);
        assert_eq!(starts(&o), vec![("acme-web".into(), 125, "sonnet".into())]);

        // A 123 mudou (corpo, rótulo ou prioridade): impressão nova, espera.
        s.projects[0].items[1].fingerprint = "fp-123-v2".into();
        s.projects[0].items.remove(0);
        s.analysis_states
            .insert("s-analise".into(), SessionState::Idle);
        let o = decide(&s);
        assert_eq!(skip_of_item(&o, 123).as_deref(), Some("analysis_pending"));
        assert!(starts(&o).is_empty());
        assert!(o
            .actions
            .iter()
            .any(|a| matches!(a, Action::ContinueAnalysis { .. })));
    }

    #[test]
    fn bloqueante_nao_pronto_tira_da_fila_ate_mesclar() {
        let mut s = snap(SAB);
        s.projects[0].deps = vec![dep(123, 120, "analysis", false)];
        s.projects[0].blockers.insert(120, BlockerState::Pending);
        let o = decide(&s);
        assert!(starts(&o).is_empty());
        let q = &o.projects[0].queue[0];
        assert_eq!(q.skip.as_deref(), Some("blocked_by"));
        assert_eq!(q.blocked_by, vec![120]);
        assert_eq!(q.dependencies[0].state, BlockerState::Pending);
        assert_eq!(o.projects[0].blocked.as_deref(), Some("no_eligible"));

        // Mesclado na wip (ou concluído): entra na fila.
        s.projects[0].blockers.insert(120, BlockerState::Ready);
        let o = decide(&s);
        assert_eq!(starts(&o), vec![("acme-web".into(), 123, "opus".into())]);
        assert!(o.projects[0].queue[0].blocked_by.is_empty());

        // Removida à mão: não bloqueia, mesmo com o bloqueante pendente.
        s.projects[0].deps = vec![dep(123, 120, "manual", true)];
        s.projects[0].blockers.insert(120, BlockerState::Pending);
        let o = decide(&s);
        assert_eq!(starts(&o).len(), 1);
        assert!(o.projects[0].queue[0].dependencies[0].removed);

        // Ambiente sem a análise: o aviso sai, e as arestas valem do mesmo
        // jeito.
        assert!(o.projects[0].analysis_unsupported);
        assert!(o
            .notices
            .iter()
            .any(|(k, r)| *k == EV_ANALYSIS_UNSUPPORTED && r.scope == "analysis:env-auto"));
    }

    #[test]
    fn bloqueante_desconhecido_mantem_o_bloqueio_e_avisa() {
        let mut s = snap(SAB);
        s.projects[0].deps = vec![dep(123, 999, "analysis", false)];
        let o = decide(&s);
        assert!(starts(&o).is_empty());
        let q = &o.projects[0].queue[0];
        assert_eq!(q.skip.as_deref(), Some("blocked_by"));
        assert_eq!(q.blocked_by, vec![999]);
        assert_eq!(q.dependencies[0].state, BlockerState::Unknown);
        let (_, n) = o
            .notices
            .iter()
            .find(|(k, _)| *k == EV_DEPENDENCY_UNKNOWN)
            .unwrap();
        assert_eq!(n.scope, "deps:acme/acme-web");
        assert!(n.message.contains("#123 ← #999"), "{}", n.message);
    }

    #[test]
    fn ciclo_tira_as_issues_da_fila_e_gera_o_evento() {
        let mut s = snap(SAB);
        s.projects[0].items = vec![
            item(123, Some(Priority::P1), Some("opus"), true),
            item(125, Some(Priority::P1), Some("opus"), true),
            item(126, Some(Priority::P2), Some("opus"), true),
            item(127, Some(Priority::P3), Some("opus"), true),
        ];
        s.projects[0].deps = vec![
            dep(123, 125, "analysis", false),
            dep(125, 123, "manual", false),
            dep(126, 125, "analysis", false),
        ];
        for n in [123, 125] {
            s.projects[0].blockers.insert(n, BlockerState::Pending);
        }
        let o = decide(&s);
        assert_eq!(skip_of_item(&o, 123).as_deref(), Some("dependency_cycle"));
        assert_eq!(skip_of_item(&o, 125).as_deref(), Some("dependency_cycle"));
        // Depende do ciclo sem fazer parte: só bloqueada.
        assert_eq!(skip_of_item(&o, 126).as_deref(), Some("blocked_by"));
        // A fila segue para a próxima elegível.
        assert_eq!(starts(&o), vec![("acme-web".into(), 127, "opus".into())]);
        let (_, n) = o
            .notices
            .iter()
            .find(|(k, _)| *k == EV_DEPENDENCY_CYCLE)
            .unwrap();
        assert_eq!(n.scope, "cycle:acme/acme-web");
        assert!(n.message.contains("#123, #125"), "{}", n.message);
        // A pessoa remove uma aresta: o ciclo some.
        s.projects[0].deps[1].removed = true;
        let o = decide(&s);
        assert!(!o.notices.iter().any(|(k, _)| *k == EV_DEPENDENCY_CYCLE));
        assert_eq!(starts(&o), vec![("acme-web".into(), 125, "opus".into())]);
    }

    #[test]
    fn sessao_de_analise_que_some_ou_para_falha_e_reinicia_depois() {
        // Sumiu antes de terminar o planejamento.
        let mut s = snap_analise(SAB);
        rodada(
            &mut s,
            Some(("s-analise", SessionState::Exited)),
            None,
            SAB - H,
        );
        let o = decide(&s);
        let fail: Vec<_> = o
            .actions
            .iter()
            .filter_map(|a| match a {
                Action::FailAnalysis { round, kill, .. } => Some((round, kill)),
                _ => None,
            })
            .collect();
        assert_eq!(fail.len(), 1);
        assert_eq!(fail[0].0.session_id, None);
        assert_eq!(fail[0].0.failed_at, Some(SAB));
        assert_eq!(fail[0].1, &None);
        // No mesmo ciclo não reinicia: espera o prazo.
        assert!(analysis_starts(&o).is_empty());
        assert!(starts(&o).is_empty());
        assert_eq!(o.projects[0].blocked.as_deref(), Some("analysis"));

        // Rodada sem sessão, dentro do prazo: espera; depois, reinicia.
        let mut s = snap_analise(SAB + 5 * MIN);
        rodada(&mut s, None, None, SAB - H);
        s.analyses.get_mut(&1).unwrap().round.failed_at = Some(SAB);
        let o = decide(&s);
        assert!(analysis_starts(&o).is_empty());
        assert_eq!(o.projects[0].analysis.as_ref().unwrap().state, "failed");
        s.now = SAB + ANALYSIS_RETRY_MS;
        s.accounts
            .insert("conta-a".into(), account(10.0, 10.0, s.now));
        let o = decide(&s);
        let a = analysis_starts(&o);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].round.opened_at, SAB - H);
        assert_eq!(a[0].reason, ANALYSIS_PLANNING);

        // Parada ociosa com a reanálise pendente: encerrada.
        let mut s = snap_analise(SAB);
        rodada(
            &mut s,
            Some(("s-analise", SessionState::Idle)),
            Some(SAB - 2 * H),
            SAB - 3 * H,
        );
        {
            let r = &mut s.analyses.get_mut(&1).unwrap().round;
            r.pending = BTreeMap::from([(123, "fp-123".to_string())]);
            r.sent_at = Some(SAB - ANALYSIS_STALL_MS);
        }
        let o = decide(&s);
        assert!(o.actions.iter().any(|a| matches!(
            a,
            Action::FailAnalysis { kill: Some(k), .. } if k == "s-analise"
        )));
        // Planejada: a falha não trava as já analisadas... e a 123, que
        // estava na reanálise, espera a próxima sessão.
        assert_eq!(skip_of_item(&o, 123).as_deref(), Some("analysis_pending"));

        // Iniciada há pouco, ainda desconhecida do hub: conta como viva.
        let mut s = snap_analise(SAB);
        rodada(&mut s, None, None, SAB - MIN);
        s.analyses.get_mut(&1).unwrap().round.session_id = Some("s-nova".into());
        let o = decide(&s);
        assert!(!o
            .actions
            .iter()
            .any(|a| matches!(a, Action::FailAnalysis { .. })));
    }

    #[test]
    fn versao_do_ambiente_desconhecida_espera_e_modo_desligado_fecha_a_rodada() {
        let mut s = snap_analise(SAB);
        s.envs.get_mut("env-auto").unwrap().analysis = None;
        let o = decide(&s);
        assert!(starts(&o).is_empty());
        assert!(analysis_starts(&o).is_empty());
        assert_eq!(o.projects[0].blocked.as_deref(), Some("analysis"));

        // Rodada de um projeto que saiu do modo autônomo: fecha.
        let mut s = snap_analise(SAB);
        rodada(
            &mut s,
            Some(("s-analise", SessionState::Idle)),
            Some(SAB),
            SAB,
        );
        s.projects.clear();
        let o = decide(&s);
        assert_eq!(closes(&o), 1);

        // Sem issue na fila: nenhuma análise (nada a planejar).
        let mut s = snap_analise(SAB);
        s.projects[0].items = vec![item(124, Some(Priority::P0), None, true)];
        let o = decide(&s);
        assert!(analysis_starts(&o).is_empty());
        assert_eq!(o.projects[0].blocked.as_deref(), Some("no_eligible"));
    }

    #[test]
    fn analise_conta_como_o_inicio_da_conta_no_ciclo() {
        let mut s = snap_analise(SAB);
        s.settings.max_sessions_per_env = 5;
        let mut p2 = project(
            "acme-api",
            "env-auto",
            vec![item(7, Some(Priority::P0), Some("opus"), true)],
        );
        p2.project_id = 2;
        p2.analysed.insert(7, "fp-7".into());
        s.projects.push(p2);
        let o = decide(&s);
        // O primeiro projeto inicia a análise; o segundo (já analisado, mas
        // sem rodada nesta janela) espera a vez da conta.
        assert_eq!(analysis_starts(&o).len(), 1);
        assert!(starts(&o).is_empty());
        assert_eq!(o.projects[1].blocked.as_deref(), Some("account_turn"));
    }
}
