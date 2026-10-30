use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("valor inválido para {kind}: {value}")]
pub struct ParseEnumError {
    pub kind: &'static str,
    pub value: String,
}

/// Enum persistido como string. Mantém `as_str`, `Display`, `FromStr` e serde alinhados.
macro_rules! str_enum {
    ($name:ident, $kind:literal, { $($variant:ident => $s:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS))]
        pub enum $name {
            $( #[serde(rename = $s)] $variant ),+
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = ParseEnumError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($s => Ok($name::$variant),)+
                    _ => Err(ParseEnumError { kind: $kind, value: s.to_string() }),
                }
            }
        }
    };
}

str_enum!(WorkItemKind, "work_item_kind", { Epic => "epic", Issue => "issue" });

str_enum!(WorkItemStatus, "work_item_status", {
    Backlog => "backlog",
    EmExecucao => "em_execucao",
    AguardandoVoce => "aguardando_voce",
    EmRevisao => "em_revisao",
    // Só sub-issue: PR mesclado na branch do épico, à espera do merge do
    // épico na base (épico #456).
    AguardandoEpico => "aguardando_epico",
    Concluido => "concluido",
    Bloqueado => "bloqueado",
    Pausado => "pausado",
});

// Tipo do projeto: um repositório do GitHub, ou o sentinela das sessões do
// gestor de projetos (spec do gestor §2.1), que não é repositório.
str_enum!(ProjectKind, "project_kind", { Github => "github", Gestor => "gestor" });

#[allow(clippy::derivable_impls)]
impl Default for ProjectKind {
    fn default() -> Self {
        ProjectKind::Github
    }
}

/// Chave reservada do projeto sentinela do gestor (`_ctower/gestor`). Nomes
/// do GitHub não começam com `_`, então não colide com um repositório real.
pub const GESTOR_OWNER: &str = "_ctower";
pub const GESTOR_REPO: &str = "gestor";

// Prioridade de um work item, gravada só no hub (spec do gestor §4.1).
// `P0` é a mais urgente; a ausência (`None`) é "não triado", diferente de `P2`.
// A ordem das variantes é a da urgência, e a das strings também.
str_enum!(Priority, "priority", { P0 => "P0", P1 => "P1", P2 => "P2", P3 => "P3" });

// Proposta do gestor (spec do gestor §8): o que muda e em que pé está. Só o
// hub marca `obsoleta`; `aprovada` e `rejeitada` vêm da UI do hub.
str_enum!(ProposalKind, "proposal_kind", { Priority => "priority", Status => "status" });
str_enum!(ProposalState, "proposal_state", {
    Pendente => "pendente",
    Aprovada => "aprovada",
    Rejeitada => "rejeitada",
    Obsoleta => "obsoleta",
});

// Origem de uma versão das regras do agente (issue #357, spec do Jira e
// GitLab §10.2): edição do usuário na UI do hub, proposta aprovada ou
// importação de um `CLAUDE.md`.
str_enum!(RuleSource, "rule_source", {
    Usuario => "usuario",
    Proposta => "proposta",
    Importacao => "importacao",
});

str_enum!(SessionState, "session_state", {
    Starting => "starting",
    Running => "running",
    Idle => "idle",
    Exited => "exited",
    Interrupted => "interrupted",
    Archived => "archived",
});

str_enum!(Tool, "tool", { Claude => "claude", Codex => "codex" });

// Como o agente executa o trabalho: direto na sessão (`native`) ou delegando
// cada tarefa a subagentes (`subagents`).
str_enum!(SessionMode, "session_mode", { Native => "native", Subagents => "subagents" });

// `str_enum!` não aceita `#[default]` na variante.
#[allow(clippy::derivable_impls)]
impl Default for SessionMode {
    fn default() -> Self {
        SessionMode::Native
    }
}

/// Modelos do Claude Code aceitos em `--model` (aliases da versão mais nova
/// de cada família). Espelha `ui/src/settings/session-options.ts`.
pub const CLAUDE_MODELS: &[&str] = &["fable", "opus", "sonnet", "haiku"];

str_enum!(DecisionOrigin, "decision_origin", {
    Autonoma => "autonoma",
    Orientada => "orientada",
    Manual => "manual",
});

impl SessionState {
    /// Sessão com processo (supostamente) vivo.
    pub fn is_live(self) -> bool {
        matches!(
            self,
            SessionState::Starting | SessionState::Running | SessionState::Idle
        )
    }

    pub fn can_transition_to(self, next: SessionState) -> bool {
        use SessionState::*;
        matches!(
            (self, next),
            (Starting, Running | Idle | Exited | Interrupted)
                | (Running, Idle | Exited | Interrupted)
                | (Idle, Running | Exited | Interrupted)
                | (Interrupted, Archived)
                | (Exited, Archived)
        )
    }
}

/// Eventos automáticos que movem o status de um work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkItemTrigger {
    SessionActive,
    NeedsHuman,
    PrOpened,
    PrMerged,
    /// PR da sub-issue mesclado na branch do épico (épico #456).
    SubIssuePrMerged,
    /// PR do épico mesclado na base: conclui a sub-issue que o aguardava.
    EpicMerged,
}

impl WorkItemTrigger {
    /// Nome persistido como `cause` no evento `status_changed`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionActive => "session_active",
            Self::NeedsHuman => "needs_human",
            Self::PrOpened => "pr_opened",
            Self::PrMerged => "pr_merged",
            Self::SubIssuePrMerged => "sub_issue_pr_merged",
            Self::EpicMerged => "epic_merged",
        }
    }
}

/// Novo status para um gatilho automático, ou `None` se nada muda.
///
/// O gatilho vence status manuais (`bloqueado`, `pausado`); um item `concluido`
/// só é reaberto por atividade de sessão. Um item `aguardando_epico` só sai
/// por atividade de sessão (volta a `em_execucao`) ou pelo merge do épico
/// (`EpicMerged`): outro PR, aberto ou mesclado, e um pedido de atenção não o
/// tiram dali. `EpicMerged` só move quem aguarda o épico.
pub fn next_status(current: WorkItemStatus, trigger: WorkItemTrigger) -> Option<WorkItemStatus> {
    use WorkItemStatus as S;
    use WorkItemTrigger as T;
    let target = match trigger {
        T::SessionActive => S::EmExecucao,
        T::NeedsHuman => S::AguardandoVoce,
        T::PrOpened => S::EmRevisao,
        T::PrMerged => S::Concluido,
        T::SubIssuePrMerged => S::AguardandoEpico,
        T::EpicMerged => S::Concluido,
    };
    let aceito = match current {
        S::Concluido => trigger == T::SessionActive,
        S::AguardandoEpico => matches!(trigger, T::SessionActive | T::EpicMerged),
        _ => trigger != T::EpicMerged,
    };
    (aceito && target != current).then_some(target)
}

str_enum!(PrState, "pr_state", {
    Open => "open",
    Closed => "closed",
    Merged => "merged",
});

/// Gatilho de uma mudança no estado do PR da branch do work item, ou `None`.
///
/// Só a transição dispara: um PR que continua aberto não reaplica `PrOpened`
/// (uma mudança manual de status prevalece até o próximo gatilho).
pub fn pr_trigger(prev: Option<PrState>, new: PrState) -> Option<WorkItemTrigger> {
    match new {
        _ if prev == Some(new) => None,
        PrState::Open => Some(WorkItemTrigger::PrOpened),
        PrState::Merged => Some(WorkItemTrigger::PrMerged),
        PrState::Closed => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enums_ida_e_volta_por_string() {
        for v in WorkItemKind::ALL {
            assert_eq!(v.as_str().parse::<WorkItemKind>().unwrap(), *v);
        }
        for v in WorkItemStatus::ALL {
            assert_eq!(v.as_str().parse::<WorkItemStatus>().unwrap(), *v);
        }
        for v in SessionState::ALL {
            assert_eq!(v.as_str().parse::<SessionState>().unwrap(), *v);
        }
        for v in Tool::ALL {
            assert_eq!(v.as_str().parse::<Tool>().unwrap(), *v);
        }
        for v in SessionMode::ALL {
            assert_eq!(v.as_str().parse::<SessionMode>().unwrap(), *v);
        }
        for v in DecisionOrigin::ALL {
            assert_eq!(v.as_str().parse::<DecisionOrigin>().unwrap(), *v);
        }
        for v in PrState::ALL {
            assert_eq!(v.as_str().parse::<PrState>().unwrap(), *v);
        }
    }

    #[test]
    fn gatilho_de_pr_so_na_transicao() {
        use PrState::*;
        use WorkItemTrigger::*;
        let casos = [
            (None, Open, Some(PrOpened)),
            (Some(Closed), Open, Some(PrOpened)),
            (Some(Open), Open, None),
            (None, Merged, Some(PrMerged)),
            (Some(Open), Merged, Some(PrMerged)),
            (Some(Merged), Merged, None),
            (None, Closed, None),
            (Some(Open), Closed, None),
        ];
        for (prev, new, esperado) in casos {
            assert_eq!(pr_trigger(prev, new), esperado, "{prev:?} → {new:?}");
        }
    }

    #[test]
    fn string_invalida_gera_erro_com_contexto() {
        let err = "xpto".parse::<Tool>().unwrap_err();
        assert_eq!(err.kind, "tool");
        assert_eq!(err.value, "xpto");
    }

    #[test]
    fn serde_usa_as_mesmas_strings() {
        assert_eq!(
            serde_json::to_string(&WorkItemStatus::AguardandoVoce).unwrap(),
            "\"aguardando_voce\""
        );
        let s: SessionState = serde_json::from_str("\"interrupted\"").unwrap();
        assert_eq!(s, SessionState::Interrupted);
    }

    #[test]
    fn sessao_transicoes_validas() {
        use SessionState::*;
        assert!(Starting.can_transition_to(Running));
        assert!(Starting.can_transition_to(Idle));
        assert!(Running.can_transition_to(Idle));
        assert!(Idle.can_transition_to(Running));
        assert!(Running.can_transition_to(Exited));
        assert!(Idle.can_transition_to(Interrupted));
        assert!(Interrupted.can_transition_to(Archived));
        assert!(Exited.can_transition_to(Archived));
    }

    #[test]
    fn sessao_transicoes_invalidas() {
        use SessionState::*;
        assert!(!Exited.can_transition_to(Running));
        assert!(!Archived.can_transition_to(Running));
        assert!(!Interrupted.can_transition_to(Running));
        assert!(!Running.can_transition_to(Starting));
        assert!(!Running.can_transition_to(Archived));
    }

    #[test]
    fn sessao_viva() {
        use SessionState::*;
        assert!(Starting.is_live() && Running.is_live() && Idle.is_live());
        assert!(!Exited.is_live() && !Interrupted.is_live() && !Archived.is_live());
    }

    #[test]
    fn gatilhos_definem_status() {
        use WorkItemStatus::*;
        assert_eq!(
            next_status(Backlog, WorkItemTrigger::SessionActive),
            Some(EmExecucao)
        );
        assert_eq!(
            next_status(EmExecucao, WorkItemTrigger::NeedsHuman),
            Some(AguardandoVoce)
        );
        assert_eq!(
            next_status(AguardandoVoce, WorkItemTrigger::SessionActive),
            Some(EmExecucao)
        );
        assert_eq!(
            next_status(EmExecucao, WorkItemTrigger::PrOpened),
            Some(EmRevisao)
        );
        assert_eq!(
            next_status(EmRevisao, WorkItemTrigger::PrMerged),
            Some(Concluido)
        );
    }

    #[test]
    fn gatilho_sem_mudanca_retorna_none() {
        use WorkItemStatus::*;
        assert_eq!(
            next_status(EmExecucao, WorkItemTrigger::SessionActive),
            None
        );
    }

    #[test]
    fn gatilho_vence_status_manual() {
        use WorkItemStatus::*;
        assert_eq!(
            next_status(Pausado, WorkItemTrigger::SessionActive),
            Some(EmExecucao)
        );
        assert_eq!(
            next_status(Bloqueado, WorkItemTrigger::NeedsHuman),
            Some(AguardandoVoce)
        );
    }

    #[test]
    fn sub_issue_aguarda_o_epico_e_conclui_no_merge_dele() {
        use WorkItemStatus::*;
        assert_eq!(
            next_status(EmExecucao, WorkItemTrigger::SubIssuePrMerged),
            Some(AguardandoEpico)
        );
        // Vence status manual, como os outros gatilhos.
        assert_eq!(
            next_status(Pausado, WorkItemTrigger::SubIssuePrMerged),
            Some(AguardandoEpico)
        );
        assert_eq!(
            next_status(AguardandoEpico, WorkItemTrigger::EpicMerged),
            Some(Concluido)
        );
        assert_eq!(
            next_status(AguardandoEpico, WorkItemTrigger::SubIssuePrMerged),
            None
        );
        // Concluída não volta a aguardar o épico.
        assert_eq!(
            next_status(Concluido, WorkItemTrigger::SubIssuePrMerged),
            None
        );
        assert_eq!(next_status(Concluido, WorkItemTrigger::EpicMerged), None);
    }

    #[test]
    fn merge_do_epico_so_conclui_quem_o_aguarda() {
        use WorkItemStatus::*;
        for s in [
            Backlog,
            EmExecucao,
            AguardandoVoce,
            EmRevisao,
            Bloqueado,
            Pausado,
        ] {
            assert_eq!(next_status(s, WorkItemTrigger::EpicMerged), None, "{s}");
        }
    }

    #[test]
    fn aguardando_epico_nao_volta_para_revisao_nem_atencao() {
        use WorkItemStatus::*;
        assert_eq!(
            next_status(AguardandoEpico, WorkItemTrigger::PrOpened),
            None
        );
        assert_eq!(
            next_status(AguardandoEpico, WorkItemTrigger::NeedsHuman),
            None
        );
        // Só o merge do épico conclui: o de outro PR da sub-issue, não.
        assert_eq!(
            next_status(AguardandoEpico, WorkItemTrigger::PrMerged),
            None
        );
    }

    #[test]
    fn aguardando_epico_reabre_com_atividade_de_sessao() {
        use WorkItemStatus::*;
        assert_eq!(
            next_status(AguardandoEpico, WorkItemTrigger::SessionActive),
            Some(EmExecucao)
        );
    }

    #[test]
    fn concluido_so_reabre_com_atividade_de_sessao() {
        use WorkItemStatus::*;
        assert_eq!(next_status(Concluido, WorkItemTrigger::PrOpened), None);
        assert_eq!(next_status(Concluido, WorkItemTrigger::NeedsHuman), None);
        assert_eq!(
            next_status(Concluido, WorkItemTrigger::SessionActive),
            Some(EmExecucao)
        );
    }
}
