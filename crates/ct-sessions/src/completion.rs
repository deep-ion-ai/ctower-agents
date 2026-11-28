//! Sinal único de conclusão de um work item (épico #284): usado pela
//! recuperação das sessões (o item concluído não tem o que retomar) e pela
//! limpeza do ambiente (`ct-cleanup`), que o re-exporta.

use crate::{Result, PROJECT_WORK_ITEM};
use ct_core::{PrState, WorkItemStatus};
use ct_ledger::{Ledger, Project, WorkItem};
use serde::{Deserialize, Serialize};

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
