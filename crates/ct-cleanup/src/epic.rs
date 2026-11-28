//! Remoção do worktree de uma sub-issue da execução de épico (#298) depois
//! do merge na `wip`: o hub pede `worktree.remove` sem esperar a limpeza
//! periódica, com as mesmas proteções da limpeza ([`WorktreeStep`]).

use crate::{CleanupStep, CleanupTarget, StepContext, StepState, WorktreeStep};
use ct_sessions::config::CleanupConfig;
use ct_sessions::{Result, Sessions, SessionsError};
use serde::Serialize;

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

/// Remoção do worktree da execução de épico nas [`Sessions`].
pub trait EpicWorktree {
    /// Remove o worktree de uma sub-issue da execução de épico já mesclada
    /// na `wip`, sem esperar a limpeza periódica: a etapa "worktree" da
    /// limpeza ([`WorktreeStep`]), sem `--force`, recusando com sessão viva,
    /// mudança não commitada ou commit não enviado. A branch local e a
    /// remota ficam para a limpeza normal. Só para work item marcado com
    /// [`ct_sessions::epic_run::EPIC_RUN_SESSION`].
    fn remove_epic_worktree(
        &self,
        work_item_id: i64,
        config: &CleanupConfig,
        now: i64,
    ) -> Result<WorktreeRemoval>;
}

impl EpicWorktree for Sessions {
    fn remove_epic_worktree(
        &self,
        work_item_id: i64,
        config: &CleanupConfig,
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
    use super::EpicWorktree;
    use ct_core::SessionState;
    use ct_sessions::config::CleanupConfig;
    use ct_sessions::test_support::{git_in, FakeClaude, TestEnv};
    use ct_sessions::SessionsError;
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
