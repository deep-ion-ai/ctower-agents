//! Sessões de sub-issue da execução de épico (#298, épico #295): o hub as
//! inicia com `session.start { …, epic_run: { base } }` a partir da `wip`
//! do épico ([`Sessions::start_epic_run`]), marcadas com o evento
//! [`EPIC_RUN_SESSION`] no work item (a sessão aceita texto no
//! `session.continue`). Depois do merge da sub-issue na `wip`, o hub pede a
//! remoção do worktree ([`Sessions::remove_epic_worktree`]), com as mesmas
//! proteções da limpeza ([`WorktreeStep`]).

use crate::cleanup::{CleanupStep, CleanupTarget, StepContext, StepState};
use crate::cleanup_worktree::WorktreeStep;
use crate::{Result, Sessions, SessionsError};
use ct_ledger::Session;
use serde::Serialize;

/// Evento que marca o work item da sub-issue iniciada pela execução de
/// épico (payload: `base`).
pub const EPIC_RUN_SESSION: &str = "epic_run_session";

/// Resultado de `worktree.remove`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct WorktreeRemoval {
    /// O worktree saiu (ou já não existia).
    pub removed: bool,
    /// Caminho do worktree, se o work item tinha um.
    pub worktree: Option<String>,
    /// Por que ficou (sessão viva, mudança não commitada, commit não
    /// enviado...).
    pub reason: Option<String>,
}

impl Sessions {
    /// A sessão é de uma sub-issue iniciada pela execução de épico.
    pub fn is_epic_run_session(&self, s: &Session) -> Result<bool> {
        self.is_epic_run_item(s.work_item_id)
    }

    fn is_epic_run_item(&self, work_item_id: i64) -> Result<bool> {
        Ok(!self
            .ledger()
            .list_events_of_kind(work_item_id, EPIC_RUN_SESSION)?
            .is_empty())
    }

    /// Remove o worktree de uma sub-issue da execução de épico já mesclada
    /// na `wip`, sem esperar a limpeza periódica: a etapa "worktree" da
    /// limpeza ([`WorktreeStep`]), sem `--force`, recusando com sessão viva,
    /// mudança não commitada ou commit não enviado. A branch local e a
    /// remota ficam para a limpeza normal. Só para work item marcado com
    /// [`EPIC_RUN_SESSION`].
    pub fn remove_epic_worktree(
        &self,
        work_item_id: i64,
        config: &crate::config::CleanupConfig,
        now: i64,
    ) -> Result<WorktreeRemoval> {
        if !self.is_epic_run_item(work_item_id)? {
            return Err(SessionsError::Invalid(format!(
                "work item {work_item_id}: worktree.remove só para sub-issue da execução de épico"
            )));
        }
        let target = {
            let l = self.ledger();
            let work_item = l.get_work_item(work_item_id)?;
            let project = l.get_project(work_item.project_id)?;
            let live_sessions = l
                .list_sessions(work_item_id)?
                .into_iter()
                .filter(|s| s.state.is_live())
                .map(|s| s.id)
                .collect();
            CleanupTarget {
                project,
                work_item,
                title: None,
                completion: None,
                last_activity_at: None,
                live_sessions,
            }
        };
        let worktree = target
            .work_item
            .worktree_path
            .clone()
            .filter(|p| !p.trim().is_empty());
        let ctx = StepContext {
            sessions: self,
            github: None,
            gitlab: None,
            config,
            now_ms: now,
            target: &target,
        };
        let step = WorktreeStep;
        let plan = step.plan(&ctx)?;
        match plan.state {
            StepState::Feita => Ok(WorktreeRemoval {
                removed: true,
                worktree,
                reason: None,
            }),
            StepState::Bloqueada => Ok(WorktreeRemoval {
                removed: false,
                worktree,
                reason: plan.reason,
            }),
            StepState::Pendente => match step.run(&ctx) {
                Ok(_) => Ok(WorktreeRemoval {
                    removed: true,
                    worktree,
                    reason: None,
                }),
                Err(SessionsError::Invalid(m)) => Ok(WorktreeRemoval {
                    removed: false,
                    worktree,
                    reason: Some(m),
                }),
                Err(e) => Err(e),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::CleanupConfig;
    use crate::test_support::{git_in, FakeClaude, TestEnv};
    use crate::SessionsError;
    use ct_core::SessionState;
    use std::fs;

    /// `TestEnv` com `origin` e a `wip/10-x` (um commit além da `develop`)
    /// enviada.
    fn env() -> TestEnv {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let origin = env.tmp.path().join("origin.git");
        let o = origin.to_string_lossy().to_string();
        git_in(env.tmp.path(), &["init", "-q", "--bare", &o]);
        git_in(&env.repo, &["remote", "add", "origin", &o]);
        git_in(&env.repo, &["push", "-q", "-u", "origin", "develop"]);
        git_in(&env.repo, &["checkout", "-q", "-b", "wip/10-x"]);
        fs::write(env.repo.join("wip.txt"), "wip\n").unwrap();
        git_in(&env.repo, &["add", "wip.txt"]);
        git_in(&env.repo, &["commit", "-q", "-m", "wip"]);
        git_in(&env.repo, &["push", "-q", "-u", "origin", "wip/10-x"]);
        git_in(&env.repo, &["checkout", "-q", "develop"]);
        env
    }

    #[test]
    fn sub_issue_sai_da_wip_aceita_texto_e_o_worktree_sai_depois() {
        let env = env();
        let s = env
            .sessions
            .start_epic_run(env.request(11, "Tela"), "wip/10-x")
            .unwrap();
        assert!(env.sessions.is_epic_run_session(&s).unwrap());
        let wt = std::path::PathBuf::from(&s.cwd);
        assert!(wt.join("wip.txt").is_file(), "a worktree saiu da wip");
        env.wait_fake_output(&s.id);
        env.sessions
            .continue_session(&s.id, Some("CI verde: mescle"))
            .unwrap();
        // Sessão viva: o worktree fica, com o motivo.
        let cfg = CleanupConfig::default();
        let r = env
            .sessions
            .remove_epic_worktree(s.work_item_id, &cfg, 1)
            .unwrap();
        assert!(!r.removed && r.reason.is_some(), "{r:?}");
        env.sessions.kill(&s.id).unwrap();
        env.wait_state(&s.id, SessionState::Exited);
        // Mudança não commitada: fica.
        fs::write(wt.join("rascunho.txt"), "x").unwrap();
        let r = env
            .sessions
            .remove_epic_worktree(s.work_item_id, &cfg, 2)
            .unwrap();
        assert!(!r.removed, "{r:?}");
        assert!(
            r.reason.as_deref().unwrap().contains("rascunho.txt"),
            "{r:?}"
        );
        assert!(wt.exists());
        // Limpo: sai do disco e do `git worktree list`.
        fs::remove_file(wt.join("rascunho.txt")).unwrap();
        let r = env
            .sessions
            .remove_epic_worktree(s.work_item_id, &cfg, 3)
            .unwrap();
        assert!(r.removed, "{r:?}");
        assert!(!wt.exists());
        assert!(ct_git::find_worktree(&env.repo, &wt).unwrap().is_none());
        // De novo: nada a fazer.
        assert!(
            env.sessions
                .remove_epic_worktree(s.work_item_id, &cfg, 4)
                .unwrap()
                .removed
        );
    }

    #[test]
    fn so_sub_issue_da_execucao() {
        let env = env();
        let s = env.sessions.start(env.request(12, "Outra")).unwrap();
        assert!(!env.sessions.is_epic_run_session(&s).unwrap());
        assert!(matches!(
            env.sessions
                .remove_epic_worktree(s.work_item_id, &CleanupConfig::default(), 1),
            Err(SessionsError::Invalid(_))
        ));
        assert!(matches!(
            env.sessions.start_epic_run(env.request(13, "x"), " "),
            Err(SessionsError::Invalid(_))
        ));
        env.sessions.kill(&s.id).ok();
    }
}
