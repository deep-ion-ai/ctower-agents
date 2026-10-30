//! Poda após o ack do hub (spec do hub como plano de controle §6.2): o
//! ledger apaga as linhas numa transação ([`ct_ledger::Ledger::prune`]) e,
//! depois do commit, saem os arquivos locais das sessões podadas
//! (`<data>/sessions/<id>` e o backup do transcript em `<data>/transcripts`).
//! Worktrees, branches e o transcript original do agente não são tocados.
//!
//! Falha ao apagar um arquivo não desfaz nada (as linhas já saíram): fica
//! em [`PruneOutcome::file_errors`] para o chamador registrar no log.

use crate::{Result, Sessions};
use ct_ledger::PruneReport;
use std::io;
use std::path::{Component, Path};

/// Resultado da poda, com os arquivos.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PruneOutcome {
    pub report: PruneReport,
    /// Diretórios de sessão e backups de transcript apagados.
    pub removed_files: usize,
    /// Arquivos que não puderam sair (caminho e erro).
    pub file_errors: Vec<String>,
}

impl Sessions {
    /// Poda o que passou de `retention_days` em relação a `now_ms` e apaga
    /// os arquivos locais das sessões podadas. Síncrono.
    pub fn prune(&self, retention_days: u32, now_ms: i64) -> Result<PruneOutcome> {
        // O lock do ledger sai antes de mexer no disco.
        let report = self.ledger().prune(retention_days, now_ms)?;
        let mut out = PruneOutcome {
            report,
            ..Default::default()
        };
        let dirs = &self.config().dirs;
        let transcripts = dirs.transcripts();
        for s in &out.report.sessions {
            // Defesa: uma sessão com PTY neste processo nunca perde a pasta.
            if self.pty(&s.id).is_some() {
                out.file_errors
                    .push(format!("sessão {} ainda viva neste daemon", s.id));
                continue;
            }
            if single_component(&s.id) {
                let dir = dirs.session_dir(&s.id);
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => out.removed_files += 1,
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => out.file_errors.push(format!("{}: {e}", dir.display())),
                }
            }
            // Só o backup que o daemon fez em `<data>/transcripts`; nada fora
            // dele (o transcript original é do agente).
            let Some(b) = s.backup_path.as_deref().map(Path::new) else {
                continue;
            };
            if b.parent() != Some(transcripts.as_path()) || b.file_name().is_none() {
                continue;
            }
            match std::fs::remove_file(b) {
                Ok(()) => out.removed_files += 1,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => out.file_errors.push(format!("{}: {e}", b.display())),
            }
        }
        Ok(out)
    }
}

/// `id` é um nome simples (sem `/`, `..` ou caminho absoluto).
fn single_component(id: &str) -> bool {
    let mut c = Path::new(id).components();
    matches!((c.next(), c.next()), (Some(Component::Normal(_)), None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeClaude, TestEnv};
    use ct_core::{now_ms, SessionState, WorkItemStatus};

    const DAY_MS: i64 = 24 * 60 * 60 * 1000;

    #[test]
    fn nomes_de_sessao_seguros() {
        assert!(single_component("0193-uuid"));
        for bad in ["", "..", "a/b", "/abs", "."] {
            assert!(!single_component(bad), "{bad}");
        }
    }

    /// Item fechado, confirmado e velho: as linhas saem e, depois, a pasta da
    /// sessão e o backup do transcript; o worktree fica. Backup fora de
    /// `<data>/transcripts` nunca é apagado.
    #[test]
    fn poda_apaga_arquivos_locais_e_mantem_o_worktree() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let a = env.sessions.start(env.request(12, "Velha")).unwrap();
        let b = env.sessions.start(env.request(13, "Fora")).unwrap();
        env.wait_state(&a.id, SessionState::Exited);
        env.wait_state(&b.id, SessionState::Exited);

        let tr = env.cfg.dirs.transcripts();
        let bk = tr.join("velha.jsonl");
        std::fs::write(&bk, "{}").unwrap();
        let fora = env.tmp.path().join("fora.jsonl");
        std::fs::write(&fora, "{}").unwrap();
        let wt = {
            let l = env.sessions.ledger();
            l.set_session_backup_path(&a.id, bk.to_str().unwrap())
                .unwrap();
            l.set_session_backup_path(&b.id, fora.to_str().unwrap())
                .unwrap();
            for s in [&a, &b] {
                l.set_work_item_status(s.work_item_id, WorkItemStatus::Concluido, "t")
                    .unwrap();
            }
            l.ensure_backfilled().unwrap();
            l.outbox_ack(i64::MAX).unwrap();
            l.get_work_item(a.work_item_id)
                .unwrap()
                .worktree_path
                .unwrap()
        };
        let dir = env.cfg.dirs.session_dir(&a.id);
        assert!(dir.is_dir());

        // Ainda no prazo: nada sai.
        let o = env.sessions.prune(30, now_ms()).unwrap();
        assert!(o.report.work_items.is_empty());
        assert!(dir.is_dir() && bk.is_file());

        let o = env.sessions.prune(30, now_ms() + 31 * DAY_MS).unwrap();
        assert_eq!(o.report.work_items.len(), 2, "{o:?}");
        assert!(o.file_errors.is_empty(), "{o:?}");
        assert!(!dir.exists() && !bk.exists());
        assert!(!env.cfg.dirs.session_dir(&b.id).exists());
        assert!(fora.is_file(), "backup fora de <data>/transcripts fica");
        assert!(Path::new(&wt).is_dir(), "worktree não é tocado");
        assert_eq!(o.removed_files, 3);
        assert!(env.sessions.ledger().get_session(&a.id).is_err());
    }

    /// Arquivo que não sai depois do commit: a poda não é desfeita, o erro
    /// volta para o log.
    #[test]
    fn falha_ao_apagar_arquivo_nao_aborta() {
        use std::os::unix::fs::PermissionsExt;
        let env = TestEnv::new(FakeClaude::exit(0));
        let a = env.sessions.start(env.request(12, "Velha")).unwrap();
        env.wait_state(&a.id, SessionState::Exited);
        {
            let l = env.sessions.ledger();
            l.set_work_item_status(a.work_item_id, WorkItemStatus::Concluido, "t")
                .unwrap();
            l.ensure_backfilled().unwrap();
            l.outbox_ack(i64::MAX).unwrap();
        }
        let dir = env.cfg.dirs.session_dir(&a.id);
        let parent = dir.parent().unwrap().to_path_buf();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o500)).unwrap();
        let o = env.sessions.prune(30, now_ms() + 31 * DAY_MS);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let o = o.unwrap();
        assert_eq!(o.report.work_items.len(), 1);
        assert_eq!(o.file_errors.len(), 1, "{o:?}");
        assert!(env.sessions.ledger().get_session(&a.id).is_err());
    }
}
