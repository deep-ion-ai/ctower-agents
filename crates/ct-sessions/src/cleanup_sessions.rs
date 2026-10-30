//! Etapa "sessões" da limpeza de um work item concluído (issue #285, épico
//! #284) e a remoção dos transcripts do Claude Code do worktree que saiu.
//!
//! [`SessionsStep`]:
//! - Sessões `interrupted`/`exited` saem da Recuperação: o `run` faz um
//!   último backup do transcript (se o original ainda existe) e as arquiva
//!   (`archived`). As linhas ficam até a poda do #179. A Recuperação já as
//!   esconde antes disso ([`crate::recoverable_sessions`]), porque a etapa
//!   só roda depois das idades mínimas e, sem `cleanup.auto`, à mão.
//! - Sessão viva nunca é encerrada sem ação explícita: a etapa fica
//!   bloqueada ("issue concluída, sessão ainda aberta") até alguém encerrá-la
//!   (`session.kill`, na aba da sessão ou em Manutenção › Limpeza). Só
//!   com `cleanup.end_idle_sessions_hours` > 0 a etapa encerra sozinha as
//!   que estão `idle` neste daemon há pelo menos esse tempo.
//! - Transcripts do agente: para cada worktree do work item que já não
//!   existe, apaga `~/.claude/projects/<slug>` se houver backup em
//!   `<data>/transcripts` de todos os transcripts da pasta
//!   (`cleanup.remove_agent_transcripts`).
//!
//! # Contrato com a etapa do worktree (#265)
//!
//! A pasta de transcripts só sai depois do worktree. A etapa do worktree
//! chama, logo depois de remover o worktree com sucesso,
//! [`remove_agent_transcripts`]`(ctx, worktree)` e junta o resultado ao seu
//! (`removed`, `kept`, `freed_bytes`). A função:
//! - não faz nada com `cleanup.remove_agent_transcripts = false`;
//! - recusa (em `kept`) se o worktree ainda existe;
//! - faz o último backup dos transcripts das sessões do worktree e só apaga
//!   se todas as entradas de todas as pastas tiverem backup em dia (tudo ou
//!   nada por worktree);
//! - nunca devolve erro por falta de backup ou falha ao apagar (vai para
//!   `kept`): o worktree já saiu e a etapa não deve falhar por isso. Só
//!   falha do ledger vira `Err`.
//!
//! Para o dry-run, o `plan` do worktree pode somar
//! [`agent_transcript_dirs`]`(ctx.sessions, worktree)` aos seus alvos. Se a
//! etapa do worktree não chamar a função (ou algo ficar), a etapa "sessões"
//! pega a pasta na próxima verificação: ela avalia os worktrees do work item
//! que já não existem.

use crate::cleanup::{CleanupStep, StepContext, StepKind, StepOutcome, StepPlan};
use crate::{Result, Sessions, SessionsError};
use ct_core::SessionState;
use ct_ledger::Session;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const H: i64 = 3_600_000;

/// Espera máxima para a sessão encerrada pela etapa sair do estado vivo.
const END_TIMEOUT: Duration = Duration::from_secs(10);

/// Nome da pasta do Claude Code para `path` em `~/.claude/projects`: cada
/// caractere fora de `[A-Za-z0-9]` vira `-` (`/home/u/wt/a.b` →
/// `-home-u-wt-a-b`). O caminho do transcript registrado pelos hooks também
/// é usado ([`agent_transcript_dirs`]), então uma mudança de regra do Claude
/// Code não faz apagar a pasta errada: no máximo deixa de achar uma.
pub fn claude_project_slug(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Pastas de transcript do agente de um worktree ([`agent_transcript_dirs`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentTranscripts {
    /// Pastas que sairiam, com o tamanho em bytes.
    pub dirs: Vec<(PathBuf, u64)>,
    /// O que impede apagar (vazio = todas podem sair).
    pub blocked: Vec<String>,
}

impl AgentTranscripts {
    pub fn bytes(&self) -> u64 {
        self.dirs.iter().map(|(_, b)| b).sum()
    }
}

/// Avalia, sem mudar nada, as pastas de `~/.claude/projects` do worktree:
/// a do slug do caminho e as que contêm os transcripts registrados das
/// sessões com esse `cwd` (só filhas diretas de `~/.claude/projects`, nunca
/// links simbólicos). Uma pasta pode sair se cada entrada dela tiver backup
/// em `<data>/transcripts`: `<id>.jsonl` exige `<data>/transcripts/<id>.jsonl`
/// e a pasta `<id>/` (dados do agente da sessão) também. Outra entrada
/// qualquer bloqueia. Sessão viva com esse `cwd` também bloqueia.
///
/// Não olha se o worktree ainda existe: quem chama decide quando apagar.
pub fn agent_transcript_dirs(sessions: &Sessions, worktree: &str) -> Result<AgentTranscripts> {
    evaluate(sessions, worktree, false)
}

/// `strict = false` (plano): o transcript de uma sessão do ledger vale com
/// qualquer backup, porque a remoção refaz o backup antes de apagar.
/// `strict = true` (na hora de apagar): o backup precisa ser tão novo quanto
/// o original.
fn evaluate(sessions: &Sessions, worktree: &str, strict: bool) -> Result<AgentTranscripts> {
    let mut out = AgentTranscripts::default();
    let Some(projects) = sessions.config().claude_projects.clone() else {
        return Ok(out);
    };
    if worktree.is_empty() {
        return Ok(out);
    }
    let backups = sessions.config().dirs.transcripts();
    let in_cwd = sessions.ledger().sessions_in_cwd(worktree)?;
    for s in in_cwd.iter().filter(|s| s.state.is_live()) {
        out.blocked
            .push(format!("sessão {} ainda aberta em {worktree}", s.id));
    }
    let known: HashSet<&str> = in_cwd
        .iter()
        .filter_map(|s| s.tool_session_id.as_deref())
        .collect();
    let mut candidates = vec![projects.join(claude_project_slug(worktree))];
    for s in &in_cwd {
        let Some(parent) = s
            .transcript_path
            .as_deref()
            .and_then(|t| Path::new(t).parent())
        else {
            continue;
        };
        if parent.parent() == Some(projects.as_path()) && !candidates.iter().any(|c| c == parent) {
            candidates.push(parent.to_path_buf());
        }
    }
    for dir in candidates {
        match fs::symlink_metadata(&dir) {
            Ok(m) if m.is_dir() => {}
            _ => continue,
        }
        let mut missing = Vec::new();
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let ft = entry.file_type()?;
            let (id, is_file) = if ft.is_file() {
                match name.strip_suffix(".jsonl") {
                    Some(id) if !id.is_empty() => (id.to_string(), true),
                    _ => {
                        missing.push(name);
                        continue;
                    }
                }
            } else if ft.is_dir() {
                (name.clone(), false)
            } else {
                missing.push(name);
                continue;
            };
            let Ok(bk) = fs::metadata(backups.join(format!("{id}.jsonl"))) else {
                missing.push(name);
                continue;
            };
            if !is_file || (!strict && known.contains(id.as_str())) {
                continue;
            }
            let fresh = match (bk.modified(), entry.metadata().and_then(|m| m.modified())) {
                (Ok(b), Ok(o)) => b >= o,
                _ => false,
            };
            if !fresh {
                missing.push(format!("{name} (backup desatualizado)"));
            }
        }
        if missing.is_empty() {
            let bytes = dir_size(&dir);
            out.dirs.push((dir, bytes));
        } else {
            missing.sort();
            out.blocked.push(format!(
                "{} sem backup em {}: {}",
                dir.display(),
                backups.display(),
                missing.join(", ")
            ));
        }
    }
    Ok(out)
}

/// Apaga as pastas de transcript do agente de um worktree que já saiu. É o
/// gancho que a etapa do worktree (#265) chama depois de removê-lo; veja o
/// contrato no topo do módulo. Tudo o que fica vai para `kept`, com o motivo.
pub fn remove_agent_transcripts(ctx: &StepContext<'_>, worktree: &str) -> Result<StepOutcome> {
    let mut o = StepOutcome::default();
    if !ctx.config.remove_agent_transcripts || worktree.is_empty() {
        return Ok(o);
    }
    if Path::new(worktree).exists() {
        o.kept.push(format!(
            "transcripts do agente de {worktree}: o worktree ainda existe"
        ));
        return Ok(o);
    }
    // Último backup: o periódico só cobre as sessões `running`.
    let in_cwd = ctx.sessions.ledger().sessions_in_cwd(worktree)?;
    for s in in_cwd.iter().filter(|s| !s.state.is_live()) {
        refresh_backup(ctx.sessions, s, &mut o)?;
    }
    let t = evaluate(ctx.sessions, worktree, true)?;
    if !t.blocked.is_empty() {
        o.kept.extend(
            t.blocked
                .into_iter()
                .map(|b| format!("transcripts do agente: {b}")),
        );
        return Ok(o);
    }
    for (dir, bytes) in t.dirs {
        match fs::remove_dir_all(&dir) {
            Ok(()) => {
                o.freed_bytes += bytes;
                o.removed
                    .push(format!("transcripts do agente {}", dir.display()));
            }
            Err(e) => o.kept.push(format!(
                "transcripts do agente {}: falha ao apagar ({e})",
                dir.display()
            )),
        }
    }
    Ok(o)
}

/// Refaz o backup do transcript da sessão se o original ainda existe. Falha
/// vira evento `transcript_backup_failed` e uma linha em `kept`.
fn refresh_backup(sessions: &Sessions, s: &Session, o: &mut StepOutcome) -> Result<()> {
    let has_original = s.tool_session_id.is_some()
        && s.transcript_path
            .as_deref()
            .is_some_and(|t| Path::new(t).is_file());
    if !has_original {
        return Ok(());
    }
    if let Err(e) = sessions.backup_session(&s.id) {
        sessions.backup_failed(s, &e)?;
        o.kept.push(format!(
            "sessão {}: último backup do transcript falhou ({e})",
            s.id
        ));
    }
    Ok(())
}

/// Tamanho de uma pasta, sem seguir links simbólicos.
pub(crate) fn dir_size(dir: &Path) -> u64 {
    let Ok(rd) = fs::read_dir(dir) else {
        return 0;
    };
    rd.flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Etapa "sessões" (#285); veja o topo do módulo.
pub struct SessionsStep;

impl SessionsStep {
    /// A etapa pode encerrar a sessão viva sozinha: opção ligada, sessão
    /// `idle` há pelo menos `end_idle_sessions_hours` e com PTY neste daemon.
    fn can_end(ctx: &StepContext<'_>, s: &Session) -> bool {
        let hours = ctx.config.end_idle_sessions_hours;
        hours > 0
            && s.state == SessionState::Idle
            && ctx.now_ms.saturating_sub(s.last_event_at)
                >= i64::try_from(hours).unwrap_or(i64::MAX).saturating_mul(H)
            && ctx.sessions.pty(&s.id).is_some()
    }

    /// Worktrees do work item que já não existem: o do work item e os `cwd`
    /// das sessões, menos o clone principal e a pasta do gestor (de vários
    /// work items).
    fn gone_worktrees(ctx: &StepContext<'_>, list: &[Session]) -> Vec<String> {
        let t = ctx.target;
        let skip = [
            t.project.repo_path.as_str(),
            &*ctx.sessions.config().gestor_dir.to_string_lossy(),
        ]
        .map(|p| p.trim_end_matches('/').to_string());
        let mut out: Vec<String> = Vec::new();
        let all = t
            .work_item
            .worktree_path
            .iter()
            .map(String::as_str)
            .chain(list.iter().map(|s| s.cwd.as_str()));
        for p in all {
            if p.is_empty()
                || skip.iter().any(|s| s == p.trim_end_matches('/'))
                || out.iter().any(|o| o == p)
                || Path::new(p).exists()
            {
                continue;
            }
            out.push(p.to_string());
        }
        out
    }

    fn idle_hours(ctx: &StepContext<'_>, s: &Session) -> i64 {
        ctx.now_ms.saturating_sub(s.last_event_at) / H
    }
}

/// Motivo do bloqueio com sessão viva.
fn open_reason(open: &[&Session]) -> String {
    format!(
        "issue concluída, sessão ainda aberta ({}); encerre-a na aba da sessão ou em \
         Manutenção › Limpeza, ou ligue cleanup.end_idle_sessions_hours",
        open.iter()
            .map(|s| format!("{} {}", s.id, s.state))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

impl CleanupStep for SessionsStep {
    fn kind(&self) -> StepKind {
        StepKind::Sessoes
    }

    fn plan(&self, ctx: &StepContext<'_>) -> Result<StepPlan> {
        let kind = StepKind::Sessoes;
        let list = ctx
            .sessions
            .ledger()
            .list_sessions(ctx.target.work_item.id)?;
        let (ending, open): (Vec<&Session>, Vec<&Session>) = list
            .iter()
            .filter(|s| s.state.is_live())
            .partition(|s| Self::can_end(ctx, s));
        if !open.is_empty() {
            let mut p = StepPlan::bloqueada(kind, open_reason(&open));
            p.targets = open
                .iter()
                .map(|s| format!("sessão {} ({}) aberta", s.id, s.state))
                .collect();
            return Ok(p);
        }
        let mut targets: Vec<String> = ending
            .iter()
            .map(|s| {
                format!(
                    "sessão {} ociosa há {} h: encerrar e arquivar \
                     (cleanup.end_idle_sessions_hours)",
                    s.id,
                    Self::idle_hours(ctx, s)
                )
            })
            .collect();
        targets.extend(
            list.iter()
                .filter(|s| matches!(s.state, SessionState::Interrupted | SessionState::Exited))
                .map(|s| format!("sessão {} ({}): sai da Recuperação", s.id, s.state)),
        );
        let mut bytes = 0;
        if ctx.config.remove_agent_transcripts {
            let mut blocked = Vec::new();
            for wt in Self::gone_worktrees(ctx, &list) {
                let a = agent_transcript_dirs(ctx.sessions, &wt)?;
                if !a.blocked.is_empty() {
                    blocked.extend(a.blocked);
                    continue;
                }
                bytes += a.bytes();
                targets.extend(
                    a.dirs
                        .iter()
                        .map(|(d, _)| format!("transcripts do agente {}", d.display())),
                );
            }
            if !blocked.is_empty() {
                let mut p = StepPlan::bloqueada(
                    kind,
                    format!(
                        "transcripts do agente sem backup ({}); \
                         cleanup.remove_agent_transcripts = false mantém a pasta e segue",
                        blocked.join("; ")
                    ),
                );
                p.targets = targets;
                return Ok(p);
            }
        }
        Ok(if targets.is_empty() {
            StepPlan::feita(kind)
        } else {
            StepPlan::pendente(kind, bytes, targets)
        })
    }

    fn run(&self, ctx: &StepContext<'_>) -> Result<StepOutcome> {
        let wi = ctx.target.work_item.id;
        let mut o = StepOutcome::default();
        let list = ctx.sessions.ledger().list_sessions(wi)?;
        let live: Vec<&Session> = list.iter().filter(|s| s.state.is_live()).collect();
        // Sessão viva só sai com a opção ligada (o `plan` acabou de checar;
        // aqui é a defesa contra uma mudança entre os dois).
        let open: Vec<&Session> = live
            .iter()
            .copied()
            .filter(|s| !Self::can_end(ctx, s))
            .collect();
        if !open.is_empty() {
            return Err(SessionsError::Invalid(open_reason(&open)));
        }
        for s in &live {
            ctx.sessions.kill(&s.id)?;
            tracing::info!(
                "limpeza: sessão {} encerrada (ociosa há {} h, cleanup.end_idle_sessions_hours)",
                s.id,
                Self::idle_hours(ctx, s)
            );
            o.removed.push(format!(
                "sessão {} encerrada (ociosa há {} h)",
                s.id,
                Self::idle_hours(ctx, s)
            ));
        }
        wait_stopped(ctx.sessions, &live)?;

        let list = ctx.sessions.ledger().list_sessions(wi)?;
        for s in list
            .iter()
            .filter(|s| matches!(s.state, SessionState::Interrupted | SessionState::Exited))
        {
            refresh_backup(ctx.sessions, s, &mut o)?;
            ctx.sessions.archive(&s.id)?;
            o.removed.push(format!(
                "sessão {} ({}) arquivada: saiu da Recuperação",
                s.id, s.state
            ));
        }
        if ctx.config.remove_agent_transcripts {
            for wt in Self::gone_worktrees(ctx, &list) {
                let r = remove_agent_transcripts(ctx, &wt)?;
                o.freed_bytes += r.freed_bytes;
                o.removed.extend(r.removed);
                o.kept.extend(r.kept);
            }
        }
        Ok(o)
    }
}

/// Espera as sessões encerradas saírem do estado vivo (o `on_exit` do PTY
/// grava `exited`). Prazo estourado = erro: as etapas seguintes não rodam
/// com sessão aberta.
fn wait_stopped(sessions: &Sessions, ended: &[&Session]) -> Result<()> {
    let deadline = Instant::now() + END_TIMEOUT;
    for s in ended {
        loop {
            if !sessions.ledger().get_session(&s.id)?.state.is_live() {
                break;
            }
            if Instant::now() >= deadline {
                return Err(SessionsError::Invalid(format!(
                    "sessão {} não terminou depois de encerrada",
                    s.id
                )));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::{Cleaner, CleanupScope, StepResult, StepState};
    use crate::config::CleanupConfig;
    use crate::test_support::{FakeClaude, TestEnv};
    use ct_core::{SessionMode, Tool, WorkItemKind, WorkItemStatus};
    use ct_ledger::NewSession;

    fn cfg() -> CleanupConfig {
        CleanupConfig {
            min_age_hours: 0,
            min_idle_hours: 0,
            ..CleanupConfig::default()
        }
    }

    fn concluir(env: &TestEnv, wi: i64) {
        env.sessions
            .ledger()
            .set_work_item_status(wi, WorkItemStatus::Concluido, "teste")
            .unwrap();
    }

    fn sessoes(plan: &crate::cleanup::CleanupPlan) -> &StepPlan {
        &plan.items[0].steps[0]
    }

    fn run(c: &Cleaner, env: &TestEnv, dry: bool, now: i64) -> crate::cleanup::CleanupRun {
        c.run(&env.sessions, CleanupScope::default(), dry, now)
            .unwrap()
    }

    #[test]
    fn slug_do_claude_code() {
        assert_eq!(
            claude_project_slug("/Users/u/worktrees/control-tower/feat__285-x.y"),
            "-Users-u-worktrees-control-tower-feat--285-x-y"
        );
        assert_eq!(claude_project_slug("/home/joão/wt"), "-home-jo-o-wt");
    }

    /// Critério de aceite: sessão viva de issue concluída nunca é encerrada
    /// sem ação explícita ou a opção ligada.
    #[test]
    fn sessao_viva_nunca_e_encerrada_sem_a_opcao() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let s = env.sessions.start(env.request(1, "A")).unwrap();
        env.wait_fake_output(&s.id);
        concluir(&env, s.work_item_id);
        let later = ct_core::now_ms() + 100 * H;
        // Opção desligada (padrão) bem depois, e ligada com a sessão rodando:
        // sempre bloqueada.
        let casos = [
            (cfg(), later),
            (
                CleanupConfig {
                    end_idle_sessions_hours: 1,
                    ..cfg()
                },
                later,
            ),
        ];
        for (c, at) in casos {
            let cleaner = Cleaner::standard(c);
            let plan = cleaner.plan(&env.sessions, at).unwrap();
            let st = sessoes(&plan);
            assert_eq!(st.state, StepState::Bloqueada, "{st:?}");
            assert!(
                st.reason
                    .as_deref()
                    .unwrap()
                    .starts_with("issue concluída, sessão ainda aberta"),
                "{st:?}"
            );
            assert_eq!(plan.items[0].live_sessions, vec![s.id.clone()]);
            let r = run(&cleaner, &env, false, at);
            assert_eq!(r.items[0].steps[0].result, StepResult::Bloqueada);
            // A etapa bloqueada segura as seguintes.
            assert_eq!(r.items[0].steps.len(), 1);
        }
        // Ociosa: com a opção desligada, ou ligada mas ociosa há pouco,
        // continua viva.
        env.sessions
            .ledger()
            .transition_session(&s.id, SessionState::Idle, None)
            .unwrap();
        let r = run(&Cleaner::standard(cfg()), &env, false, later);
        assert_eq!(r.items[0].steps[0].result, StepResult::Bloqueada);
        let pouco = Cleaner::standard(CleanupConfig {
            end_idle_sessions_hours: 2,
            ..cfg()
        });
        let r = run(&pouco, &env, false, ct_core::now_ms() + H);
        assert_eq!(r.items[0].steps[0].result, StepResult::Bloqueada);
        assert!(env.sessions.pty(&s.id).is_some());
        assert!(env
            .sessions
            .ledger()
            .get_session(&s.id)
            .unwrap()
            .state
            .is_live());
        // A defesa do `run`: chamado direto, recusa encerrar.
        let target = Cleaner::standard(cfg()).targets(&env.sessions).unwrap();
        let c = cfg();
        let ctx = StepContext {
            sessions: &env.sessions,
            github: None,
            gitlab: None,
            config: &c,
            now_ms: later,
            target: &target[0],
        };
        assert!(SessionsStep.run(&ctx).is_err());
        assert!(env.sessions.pty(&s.id).is_some());
        env.sessions.kill(&s.id).unwrap();
    }

    /// Com `end_idle_sessions_hours`, a ociosa há tempo suficiente é
    /// encerrada e arquivada; o dry-run só relata.
    #[test]
    fn opcao_encerra_a_ociosa() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let s = env.sessions.start(env.request(1, "A")).unwrap();
        env.wait_fake_output(&s.id);
        concluir(&env, s.work_item_id);
        env.sessions
            .ledger()
            .transition_session(&s.id, SessionState::Idle, None)
            .unwrap();
        let cleaner = Cleaner::standard(CleanupConfig {
            end_idle_sessions_hours: 2,
            ..cfg()
        });
        let now = ct_core::now_ms() + 3 * H;
        let plan = cleaner.plan(&env.sessions, now).unwrap();
        let st = sessoes(&plan);
        assert_eq!(st.state, StepState::Pendente, "{st:?}");
        assert!(st.targets[0].contains("ociosa há 3 h"), "{st:?}");
        let r = run(&cleaner, &env, true, now);
        assert_eq!(r.items[0].steps[0].result, StepResult::Simulada);
        assert!(env.sessions.pty(&s.id).is_some());

        let r = run(&cleaner, &env, false, now);
        let step = &r.items[0].steps[0];
        assert_eq!(step.result, StepResult::Feita, "{step:?}");
        assert!(step.removed[0].contains("encerrada"), "{step:?}");
        assert_eq!(
            env.sessions.ledger().get_session(&s.id).unwrap().state,
            SessionState::Archived
        );
    }

    /// Critério de aceite: sessão interrompida de issue concluída não
    /// aparece na Recuperação; a etapa a arquiva (e o dry-run não).
    #[test]
    fn interrompida_de_issue_concluida_sai_da_recuperacao() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        let mk = |n| {
            let s = env.sessions.start(env.request(n, "A")).unwrap();
            env.wait_fake_output(&s.id);
            env.sessions
                .ledger()
                .transition_session(&s.id, SessionState::Interrupted, None)
                .unwrap();
            env.sessions.kill(&s.id).unwrap();
            s
        };
        let (a, b) = (mk(1), mk(2));
        let ids =
            |v: Vec<crate::RecoveryItem>| v.into_iter().map(|i| i.session.id).collect::<Vec<_>>();
        assert_eq!(
            ids(env.sessions.recovery_list().unwrap()),
            vec![a.id.clone(), b.id.clone()]
        );
        concluir(&env, a.work_item_id);
        assert_eq!(
            ids(env.sessions.recovery_list().unwrap()),
            vec![b.id.clone()]
        );
        // A linha continua no ledger até a poda.
        assert_eq!(
            env.sessions.ledger().get_session(&a.id).unwrap().state,
            SessionState::Interrupted
        );

        let cleaner = Cleaner::standard(cfg());
        let now = ct_core::now_ms() + H;
        let plan = cleaner.plan(&env.sessions, now).unwrap();
        let st = sessoes(&plan);
        assert_eq!(st.state, StepState::Pendente);
        assert_eq!(
            st.targets,
            vec![format!("sessão {} (interrupted): sai da Recuperação", a.id)]
        );
        run(&cleaner, &env, true, now);
        assert_eq!(
            env.sessions.ledger().get_session(&a.id).unwrap().state,
            SessionState::Interrupted
        );
        let r = run(&cleaner, &env, false, now);
        assert_eq!(r.items[0].steps[0].result, StepResult::Feita);
        assert_eq!(
            env.sessions.ledger().get_session(&a.id).unwrap().state,
            SessionState::Archived
        );
        // Feito: a etapa não tem mais o que fazer.
        let plan = cleaner.plan(&env.sessions, now).unwrap();
        assert!(plan
            .items
            .iter()
            .all(|i| i.steps[0].state == StepState::Feita));
    }

    /// Sessão `exited` de um worktree, com o transcript em
    /// `<claude_projects>/<slug>/<tool_id>.jsonl`.
    struct Wt {
        env: TestEnv,
        wi: i64,
        session: String,
        worktree: PathBuf,
        dir: PathBuf,
    }

    fn wt() -> Wt {
        let env = TestEnv::new(FakeClaude::exit(0));
        let worktree = env.tmp.path().join("wt").join("demo").join("feat__9-x");
        fs::create_dir_all(&worktree).unwrap();
        let wt_str = worktree.to_string_lossy().to_string();
        let dir = env
            .cfg
            .claude_projects
            .clone()
            .unwrap()
            .join(claude_project_slug(&wt_str));
        fs::create_dir_all(dir.join("u-1").join("subagents")).unwrap();
        fs::write(dir.join("u-1.jsonl"), "{\"a\":1}\n").unwrap();
        fs::write(dir.join("u-1").join("subagents").join("x.jsonl"), "{}\n").unwrap();
        let (wi, session) = {
            let l = env.sessions.ledger();
            let wi = l
                .ensure_work_item(env.project_id, 9, WorkItemKind::Issue)
                .unwrap();
            let s = l
                .create_session(&NewSession {
                    work_item_id: wi.id,
                    tool: Tool::Claude,
                    cwd: wt_str.clone(),
                    launch_cmd: vec!["claude".into()],
                    model: None,
                    mode: SessionMode::Native,
                })
                .unwrap();
            l.set_session_tool_info(&s.id, "u-1", &dir.join("u-1.jsonl").to_string_lossy())
                .unwrap();
            l.transition_session(&s.id, SessionState::Exited, Some(0))
                .unwrap();
            l.set_work_item_status(wi.id, WorkItemStatus::Concluido, "teste")
                .unwrap();
            (wi.id, s.id)
        };
        Wt {
            env,
            wi,
            session,
            worktree,
            dir,
        }
    }

    /// Critério de aceite: a pasta de transcripts sai com o worktree, só
    /// com backup de todos os transcripts; o dry-run lista e não apaga.
    #[test]
    fn transcripts_saem_com_o_worktree_e_so_com_backup() {
        let w = wt();
        let cleaner = Cleaner::standard(cfg());
        let now = ct_core::now_ms() + H;
        // Worktree ainda existe: só arquiva a sessão (o último backup sai
        // junto), e a pasta fica.
        let r = run(&cleaner, &w.env, false, now);
        assert_eq!(r.items[0].steps[0].result, StepResult::Feita);
        assert!(w.dir.is_dir());
        let s = w.env.sessions.ledger().get_session(&w.session).unwrap();
        assert_eq!(s.state, SessionState::Archived);
        let backup = PathBuf::from(s.backup_path.unwrap());
        assert!(backup.is_file());
        // Nada mais a fazer: o work item sai do plano.
        let plan = cleaner.plan(&w.env.sessions, now).unwrap();
        assert!(plan.items.is_empty(), "{plan:?}");

        // Sem backup: o worktree saiu, mas a pasta fica, com o motivo.
        fs::remove_dir_all(&w.worktree).unwrap();
        fs::remove_file(&backup).unwrap();
        let plan = cleaner.plan(&w.env.sessions, now).unwrap();
        let st = sessoes(&plan);
        assert_eq!(st.state, StepState::Bloqueada, "{st:?}");
        let reason = st.reason.as_deref().unwrap();
        assert!(
            reason.contains("sem backup") && reason.contains("u-1.jsonl"),
            "{reason}"
        );
        run(&cleaner, &w.env, false, now);
        assert!(w.dir.is_dir());

        // Com backup de todas as sessões: pendente, o dry-run lista a pasta
        // e não apaga; a execução apaga.
        w.env.sessions.backup_session(&w.session).unwrap();
        let plan = cleaner.plan(&w.env.sessions, now).unwrap();
        let st = sessoes(&plan);
        assert_eq!(st.state, StepState::Pendente, "{st:?}");
        assert_eq!(
            st.targets,
            vec![format!("transcripts do agente {}", w.dir.display())]
        );
        assert!(st.bytes > 0);
        let r = run(&cleaner, &w.env, true, now);
        assert_eq!(r.items[0].steps[0].result, StepResult::Simulada);
        assert_eq!(r.items[0].steps[0].removed, st.targets);
        assert!(w.dir.is_dir());
        let r = run(&cleaner, &w.env, false, now);
        let step = &r.items[0].steps[0];
        assert_eq!(step.result, StepResult::Feita, "{step:?}");
        assert!(!w.dir.exists());
        assert_eq!(r.freed_bytes, st.bytes);
        // O backup fica (sai na poda do #179).
        assert!(backup.is_file());
        let plan = cleaner.plan(&w.env.sessions, now).unwrap();
        assert!(plan.items.is_empty() || sessoes(&plan).state == StepState::Feita);
    }

    /// Transcript de uma sessão fora do ledger (o agente rodado à mão no
    /// worktree), ou outra coisa na pasta: nada sai.
    #[test]
    fn transcript_sem_sessao_conhecida_bloqueia() {
        let w = wt();
        w.env.sessions.backup_session(&w.session).unwrap();
        fs::remove_dir_all(&w.worktree).unwrap();
        fs::write(w.dir.join("manual.jsonl"), "{}\n").unwrap();
        fs::create_dir_all(w.dir.join("memory")).unwrap();
        let cleaner = Cleaner::standard(cfg());
        let now = ct_core::now_ms() + H;
        let plan = cleaner.plan(&w.env.sessions, now).unwrap();
        let st = sessoes(&plan);
        assert_eq!(st.state, StepState::Bloqueada);
        let reason = st.reason.as_deref().unwrap();
        assert!(reason.contains("manual.jsonl, memory"), "{reason}");
        // A sessão a arquivar continua listada no que sairia.
        assert!(st.targets[0].contains(&w.session), "{st:?}");
        run(&cleaner, &w.env, false, now);
        assert!(w.dir.join("u-1.jsonl").is_file());

        // Com a opção desligada, a pasta fica e a etapa segue.
        let off = Cleaner::standard(CleanupConfig {
            remove_agent_transcripts: false,
            ..cfg()
        });
        let r = run(&off, &w.env, false, now);
        assert_eq!(r.items[0].steps[0].result, StepResult::Feita);
        assert!(w.dir.join("u-1.jsonl").is_file());
        let plan = off.plan(&w.env.sessions, now).unwrap();
        assert!(plan.items.is_empty(), "{plan:?}");
    }

    /// O gancho da etapa do worktree (#265): recusa com o worktree no disco
    /// ou a opção desligada, refaz o backup e só então apaga.
    #[test]
    fn gancho_do_worktree() {
        let w = wt();
        let target = Cleaner::standard(cfg())
            .targets(&w.env.sessions)
            .unwrap()
            .into_iter()
            .find(|t| t.work_item.id == w.wi)
            .unwrap();
        let wt_str = w.worktree.to_string_lossy().to_string();
        let call = |c: &CleanupConfig| {
            let ctx = StepContext {
                sessions: &w.env.sessions,
                github: None,
                gitlab: None,
                config: c,
                now_ms: ct_core::now_ms(),
                target: &target,
            };
            remove_agent_transcripts(&ctx, &wt_str).unwrap()
        };
        let o = call(&cfg());
        assert!(o.kept[0].contains("o worktree ainda existe"), "{o:?}");
        assert!(w.dir.is_dir());
        fs::remove_dir_all(&w.worktree).unwrap();
        let off = CleanupConfig {
            remove_agent_transcripts: false,
            ..cfg()
        };
        assert_eq!(call(&off), StepOutcome::default());
        assert!(w.dir.is_dir());
        // Ainda sem backup: a avaliação aponta o que falta.
        let a = agent_transcript_dirs(&w.env.sessions, &wt_str).unwrap();
        assert!(a.dirs.is_empty());
        assert!(a.blocked[0].contains("u-1, u-1.jsonl"), "{a:?}");
        // A remoção faz o último backup e então apaga.
        let o = call(&cfg());
        assert!(o.kept.is_empty(), "{o:?}");
        assert_eq!(
            o.removed,
            vec![format!("transcripts do agente {}", w.dir.display())]
        );
        assert!(o.freed_bytes > 0);
        assert!(!w.dir.exists());
        let s = w.env.sessions.ledger().get_session(&w.session).unwrap();
        assert!(Path::new(&s.backup_path.unwrap()).is_file());
    }
}
