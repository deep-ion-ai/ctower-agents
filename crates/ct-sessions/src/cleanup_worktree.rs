//! Etapas "worktree" e "target/" da limpeza (issue #265, épico #284).
//!
//! [`WorktreeStep`], para work item concluído: remove `wi.worktree_path` com
//! `git worktree remove` sem `--force` e roda `git worktree prune`. Fica
//! (bloqueada, com o motivo) se houver sessão viva no worktree, mudança não
//! commitada (inclusive arquivo novo fora do `.gitignore`), commit não
//! enviado ([`ct_git::worktree_blockers`]), ou se o caminho for o clone
//! principal ou a pasta do gestor. Os shells livres abertos nele (#428) não
//! seguram: são encerrados logo antes da remoção. Logo depois de remover, chama
//! [`remove_agent_transcripts`] (#285) e junta o resultado. Feita quando não
//! há worktree ou ele já não existe (e o git não guarda registro órfão
//! dele): a branch local (#286) só roda depois disso, com o worktree fora
//! do disco e do `git worktree list`.
//!
//! [`TargetStep`] apaga só o `target/` de um worktree que fica, preservando
//! o código, quando o worktree está inativo há `cleanup.target_idle_days`
//! (`0` desliga). Vale para dois casos:
//! - work item concluído cujo worktree não pode sair (ex.: mudança não
//!   commitada): o [`crate::cleanup::Cleaner`] não segura esta etapa por
//!   causa do bloqueio do worktree, porque apagar o `target/` não perde
//!   trabalho. Se o worktree pode sair, o `target/` vai junto e a etapa fica
//!   `feita`;
//! - work item ainda em andamento com worktree inativo
//!   ([`inactive_targets`]): entra no plano só com esta etapa, e só se o
//!   `target/` puder sair (o ativo, o com sessão aberta e o fora do
//!   `.gitignore` ficam de fora, para não poluir o plano).
//!
//! O `target/` só sai se for pasta de verdade (não link), ignorado pelo git
//! e sem arquivo versionado, e sem sessão viva no worktree. Atividade
//! ([`worktree_activity`]): o último evento das sessões do work item e das
//! que rodaram no worktree, e o `mtime` do worktree, do índice do git e das
//! pastas do `target/` (um `cargo build` à mão conta).

use crate::cleanup::{
    agent_transcript_dirs, human_bytes, remove_agent_transcripts, CleanupStep, CleanupTarget,
    StepContext, StepKind, StepOutcome, StepPlan, StepState,
};
use crate::cleanup_sessions::dir_size;
use crate::{Result, Sessions, SessionsError};
use ct_ledger::Session;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const DAY: i64 = 24 * 3_600_000;

/// O worktree do work item, se houver caminho.
fn worktree_of(t: &CleanupTarget) -> Option<&str> {
    t.work_item
        .worktree_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let trim = |p: &Path| p.to_string_lossy().trim_end_matches('/').to_string();
    if trim(a) == trim(b) {
        return true;
    }
    matches!(
        (fs::canonicalize(a), fs::canonicalize(b)),
        (Ok(x), Ok(y)) if x == y
    )
}

/// O caminho é o clone principal do projeto, a pasta do gestor ou a raiz
/// dos worktrees: nunca sai, nem o `target/` dele.
fn reserved(sessions: &Sessions, t: &CleanupTarget, wt: &str) -> Option<String> {
    let cfg = sessions.config();
    let wt = Path::new(wt);
    if same_dir(wt, Path::new(&t.project.repo_path)) {
        return Some("é o clone principal do projeto".into());
    }
    if same_dir(wt, &cfg.gestor_dir) {
        return Some("é a pasta do gestor".into());
    }
    if same_dir(wt, &cfg.worktrees_root) {
        return Some("é a raiz dos worktrees".into());
    }
    None
}

/// Sessões vivas no worktree (`cwd` nele ou abaixo dele) e as do work item.
fn live_in(sessions: &Sessions, t: &CleanupTarget, wt: &str) -> Result<Vec<String>> {
    let prefix = format!("{}/", wt.trim_end_matches('/'));
    let mut ids: Vec<String> = sessions
        .ledger()
        .live_sessions()?
        .into_iter()
        .filter(|s| s.cwd == wt || s.cwd.starts_with(&prefix))
        .map(|s| s.id)
        .collect();
    for id in &t.live_sessions {
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    Ok(ids)
}

fn live_reason(ids: &[String]) -> String {
    format!(
        "sessão aberta no worktree ({}); encerre-a antes",
        ids.join(", ")
    )
}

fn mtime_ms(p: &Path) -> Option<i64> {
    let t = fs::symlink_metadata(p).ok()?.modified().ok()?;
    i64::try_from(t.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()
}

/// Pasta git do worktree (o `gitdir:` do arquivo `.git`, ou a `.git`).
fn git_dir(wt: &Path) -> Option<PathBuf> {
    let dot = wt.join(".git");
    let m = fs::symlink_metadata(&dot).ok()?;
    if m.is_dir() {
        return Some(dot);
    }
    let s = fs::read_to_string(&dot).ok()?;
    let rel = s.trim().strip_prefix("gitdir:")?.trim();
    Some(wt.join(rel))
}

/// Última atividade no worktree (ms), para o `target/` de um worktree que
/// fica: o último evento das sessões do work item (`work_item_last`) e das
/// que rodaram no worktree, a criação do work item, e o `mtime` do
/// worktree, do índice e do `HEAD` do git e das pastas do `target/` até o
/// segundo nível (`target/debug`, `target/debug/deps`).
pub fn worktree_activity(sessions: &Sessions, t: &CleanupTarget, wt: &str) -> Result<Option<i64>> {
    let last = |s: &Session| match s.ended_at {
        Some(end) if !s.state.is_live() => end,
        _ => s.last_event_at,
    };
    let mut at: Vec<i64> = sessions
        .ledger()
        .sessions_in_cwd(wt)?
        .iter()
        .map(last)
        .collect();
    at.extend(t.last_activity_at);
    at.push(t.work_item.created_at);
    let p = Path::new(wt);
    at.extend(mtime_ms(p));
    if let Some(g) = git_dir(p) {
        at.extend(mtime_ms(&g.join("index")));
        at.extend(mtime_ms(&g.join("HEAD")));
    }
    let target = p.join("target");
    at.extend(mtime_ms(&target));
    if let Ok(rd) = fs::read_dir(&target) {
        for e in rd.flatten() {
            let path = e.path();
            at.extend(mtime_ms(&path));
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                if let Ok(rd) = fs::read_dir(&path) {
                    at.extend(rd.flatten().filter_map(|e| mtime_ms(&e.path())));
                }
            }
        }
    }
    Ok(at.into_iter().max())
}

/// `target/` de verdade no worktree (pasta, não link).
fn target_dir(wt: &str) -> Option<PathBuf> {
    let t = Path::new(wt).join("target");
    fs::symlink_metadata(&t)
        .is_ok_and(|m| m.is_dir())
        .then_some(t)
}

/// Etapa "worktree" (#265); veja o topo do módulo.
pub struct WorktreeStep;

impl WorktreeStep {
    /// O que impede o worktree (que existe) de sair; vazio = pode sair.
    fn blockers(ctx: &StepContext<'_>, wt: &str) -> Result<Vec<String>> {
        let live = live_in(ctx.sessions, ctx.target, wt)?;
        if !live.is_empty() {
            return Ok(vec![live_reason(&live)]);
        }
        let repo = Path::new(&ctx.target.project.repo_path);
        if !repo.is_dir() {
            return Ok(vec![format!(
                "clone do projeto não encontrado em {}",
                repo.display()
            )]);
        }
        Ok(ct_git::worktree_blockers(repo, Path::new(wt))?)
    }

    /// O worktree sumiu do disco, mas o git ainda o registra.
    fn orphan(ctx: &StepContext<'_>, wt: &str) -> Result<bool> {
        let repo = Path::new(&ctx.target.project.repo_path);
        if !repo.is_dir() {
            return Ok(false);
        }
        Ok(ct_git::find_worktree(repo, Path::new(wt))?.is_some_and(|e| e.prunable))
    }
}

impl CleanupStep for WorktreeStep {
    fn kind(&self) -> StepKind {
        StepKind::Worktree
    }

    fn plan(&self, ctx: &StepContext<'_>) -> Result<StepPlan> {
        let kind = StepKind::Worktree;
        let Some(wt) = worktree_of(ctx.target) else {
            return Ok(StepPlan::feita(kind));
        };
        if let Some(r) = reserved(ctx.sessions, ctx.target, wt) {
            return Ok(StepPlan::bloqueada(kind, format!("{wt} {r}; nunca sai")));
        }
        if !Path::new(wt).exists() {
            return Ok(if Self::orphan(ctx, wt)? {
                StepPlan::pendente(
                    kind,
                    0,
                    vec![format!(
                        "registro órfão do worktree {wt} (git worktree prune)"
                    )],
                )
            } else {
                StepPlan::feita(kind)
            });
        }
        let blockers = Self::blockers(ctx, wt)?;
        if !blockers.is_empty() {
            let mut p = StepPlan::bloqueada(kind, blockers.join("; "));
            p.targets = vec![wt.to_string()];
            return Ok(p);
        }
        let mut bytes = dir_size(Path::new(wt));
        let mut targets = vec![format!("worktree {wt}")];
        if ctx.config.remove_agent_transcripts {
            // Se algo impede, a pasta fica e a etapa "sessões" mostra o
            // motivo na próxima verificação.
            let a = agent_transcript_dirs(ctx.sessions, wt)?;
            if a.blocked.is_empty() {
                bytes += a.bytes();
                targets.extend(
                    a.dirs
                        .iter()
                        .map(|(d, _)| format!("transcripts do agente {}", d.display())),
                );
            }
        }
        Ok(StepPlan::pendente(kind, bytes, targets))
    }

    fn run(&self, ctx: &StepContext<'_>) -> Result<StepOutcome> {
        // Reavalia na hora: nada mudou desde o plano?
        let p = self.plan(ctx)?;
        if p.state != StepState::Pendente {
            return Err(SessionsError::Invalid(
                p.reason
                    .unwrap_or_else(|| "o worktree já não existe".into()),
            ));
        }
        let wt = worktree_of(ctx.target).expect("plano pendente tem worktree");
        let repo = Path::new(&ctx.target.project.repo_path);
        let mut o = StepOutcome::default();
        if !Path::new(wt).exists() {
            ct_git::prune_worktrees(repo)?;
            tracing::info!(
                "limpeza: registro órfão do worktree {wt} removido (git worktree prune)"
            );
            o.removed.push(format!(
                "registro órfão do worktree {wt} (git worktree prune)"
            ));
            return Ok(o);
        }
        let bytes = dir_size(Path::new(wt));
        // Shell livre (#428) não segura a limpeza: é encerrado antes.
        let shells = ctx.sessions.close_shells_in(wt);
        if shells > 0 {
            tracing::info!("limpeza: {shells} shell(s) no worktree {wt} encerrado(s)");
        }
        ct_git::remove_worktree(repo, Path::new(wt))?;
        if Path::new(wt).exists() {
            return Err(SessionsError::Invalid(format!(
                "git worktree remove terminou, mas {wt} continua no disco"
            )));
        }
        if let Err(e) = ct_git::prune_worktrees(repo) {
            o.kept.push(format!("git worktree prune falhou: {e}"));
        }
        tracing::info!(
            "limpeza: worktree {wt} removido ({} liberados)",
            human_bytes(bytes)
        );
        o.freed_bytes = bytes;
        o.removed.push(format!("worktree {wt}"));
        let t = remove_agent_transcripts(ctx, wt)?;
        o.freed_bytes += t.freed_bytes;
        o.removed.extend(t.removed);
        o.kept.extend(t.kept);
        Ok(o)
    }
}

/// Etapa "target/" (#265); veja o topo do módulo.
pub struct TargetStep;

impl CleanupStep for TargetStep {
    fn kind(&self) -> StepKind {
        StepKind::Target
    }

    fn plan(&self, ctx: &StepContext<'_>) -> Result<StepPlan> {
        let kind = StepKind::Target;
        let days = ctx.config.target_idle_days;
        let Some(wt) = worktree_of(ctx.target) else {
            return Ok(StepPlan::feita(kind));
        };
        if days == 0 || reserved(ctx.sessions, ctx.target, wt).is_some() {
            return Ok(StepPlan::feita(kind));
        }
        let Some(target) = target_dir(wt) else {
            return Ok(StepPlan::feita(kind));
        };
        // Concluído com o worktree livre para sair: o `target/` vai junto.
        if ctx.target.completion.is_some() && WorktreeStep::blockers(ctx, wt)?.is_empty() {
            return Ok(StepPlan::feita(kind));
        }
        let live = live_in(ctx.sessions, ctx.target, wt)?;
        if !live.is_empty() {
            return Ok(StepPlan::bloqueada(kind, live_reason(&live)));
        }
        if !ct_git::is_ignored_untracked(Path::new(wt), "target")? {
            return Ok(StepPlan::bloqueada(
                kind,
                format!(
                    "{} não está no .gitignore ou tem arquivo versionado",
                    target.display()
                ),
            ));
        }
        let idle = i64::try_from(days).unwrap_or(i64::MAX).saturating_mul(DAY);
        if let Some(at) = worktree_activity(ctx.sessions, ctx.target, wt)? {
            if ctx.now_ms.saturating_sub(at) < idle {
                return Ok(StepPlan::bloqueada(
                    kind,
                    format!(
                        "worktree com atividade há menos de {days} dia(s) \
                         (cleanup.target_idle_days)"
                    ),
                ));
            }
        }
        Ok(StepPlan::pendente(
            kind,
            dir_size(&target),
            vec![format!("{} (o código fica)", target.display())],
        ))
    }

    fn run(&self, ctx: &StepContext<'_>) -> Result<StepOutcome> {
        let p = self.plan(ctx)?;
        if p.state != StepState::Pendente {
            return Err(SessionsError::Invalid(
                p.reason.unwrap_or_else(|| "o target/ já não existe".into()),
            ));
        }
        let wt = worktree_of(ctx.target).expect("plano pendente tem worktree");
        let target = target_dir(wt).expect("plano pendente tem target/");
        let bytes = dir_size(&target);
        fs::remove_dir_all(&target)?;
        tracing::info!(
            "limpeza: {} apagado ({} liberados; o worktree fica)",
            target.display(),
            human_bytes(bytes)
        );
        Ok(StepOutcome {
            freed_bytes: bytes,
            removed: vec![format!("{} (o código fica)", target.display())],
            kept: Vec::new(),
        })
    }
}

/// Work items ainda não concluídos com o `target/` de um worktree inativo
/// há `cleanup.target_idle_days` e sem sessão viva: entram no plano só com
/// a etapa "target/" (`completion: None`). Vazio com a opção desligada.
pub(crate) fn inactive_targets(
    sessions: &Sessions,
    candidates: Vec<CleanupTarget>,
    days: u64,
    now: i64,
) -> Result<Vec<CleanupTarget>> {
    if days == 0 {
        return Ok(Vec::new());
    }
    let idle = i64::try_from(days).unwrap_or(i64::MAX).saturating_mul(DAY);
    let mut out = Vec::new();
    for mut t in candidates {
        let Some(wt) = worktree_of(&t).map(str::to_string) else {
            continue;
        };
        // O `target/` fora do `.gitignore` (outro tipo de projeto) fica de
        // fora do plano, em vez de aparecer bloqueado a cada verificação.
        if reserved(sessions, &t, &wt).is_some()
            || target_dir(&wt).is_none()
            || !live_in(sessions, &t, &wt)?.is_empty()
            || !ct_git::is_ignored_untracked(Path::new(&wt), "target").unwrap_or(false)
        {
            continue;
        }
        let at = worktree_activity(sessions, &t, &wt)?;
        if at.is_some_and(|at| now.saturating_sub(at) < idle) {
            continue;
        }
        t.last_activity_at = at;
        out.push(t);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::{Cleaner, CleanupPlan, CleanupRun, CleanupScope, StepResult};
    use crate::config::CleanupConfig;
    use crate::test_support::{git_in, FakeClaude, TestEnv};
    use ct_core::{PrState, SessionMode, Tool, WorkItemKind, WorkItemStatus};
    use ct_ledger::{GithubSnapshot, NewSession, PrRow};

    fn cfg() -> CleanupConfig {
        CleanupConfig {
            min_age_hours: 0,
            min_idle_hours: 0,
            ..CleanupConfig::default()
        }
    }

    /// `TestEnv` com `origin` (bare) e o `.gitignore` com `/target` em
    /// `develop`.
    fn env() -> TestEnv {
        let env = TestEnv::new(FakeClaude::exit(0));
        fs::write(env.repo.join(".gitignore"), "/target\n").unwrap();
        git_in(&env.repo, &["add", ".gitignore"]);
        git_in(&env.repo, &["commit", "-q", "-m", "gitignore"]);
        let origin = env.tmp.path().join("origin.git");
        let o = origin.to_string_lossy().to_string();
        git_in(env.tmp.path(), &["init", "-q", "--bare", &o]);
        git_in(&env.repo, &["remote", "add", "origin", &o]);
        git_in(&env.repo, &["push", "-q", "-u", "origin", "develop"]);
        env
    }

    /// Worktree do work item `n` (branch `feat/<n>-x`, enviada), com um
    /// `target/` de 4 KB. Devolve (work item, caminho).
    fn worktree(env: &TestEnv, n: i64) -> (i64, PathBuf) {
        let branch = format!("feat/{n}-x");
        let wt = env.tmp.path().join(format!("wt/demo/feat__{n}-x"));
        ct_git::ensure_worktree(&ct_git::WorktreeSpec {
            repo_path: &env.repo,
            branch: &branch,
            base: "develop",
            worktree_path: &wt,
        })
        .unwrap();
        git_in(&wt, &["push", "-q", "-u", "origin", &branch]);
        fs::create_dir_all(wt.join("target/debug/deps")).unwrap();
        fs::write(wt.join("target/debug/deps/lib.rlib"), vec![0u8; 4096]).unwrap();
        fs::write(wt.join("codigo.rs"), "fn main() {}\n").unwrap();
        git_in(&wt, &["add", "codigo.rs"]);
        git_in(&wt, &["commit", "-q", "-m", "codigo"]);
        git_in(&wt, &["push", "-q"]);
        let l = env.sessions.ledger();
        let wi = l
            .ensure_work_item(env.project_id, n, WorkItemKind::Issue)
            .unwrap();
        l.set_work_item_branch(wi.id, &branch, &wt.to_string_lossy())
            .unwrap();
        (wi.id, wt)
    }

    fn concluir(env: &TestEnv, wi: i64) {
        env.sessions
            .ledger()
            .set_work_item_status(wi, WorkItemStatus::Concluido, "teste")
            .unwrap();
    }

    fn step(plan: &CleanupPlan, wi: i64, kind: StepKind) -> StepPlan {
        plan.items
            .iter()
            .find(|i| i.work_item_id == wi)
            .and_then(|i| i.steps.iter().find(|s| s.kind == kind))
            .cloned()
            .unwrap_or_else(|| panic!("sem a etapa {kind} do work item {wi}: {plan:?}"))
    }

    fn run(c: &Cleaner, env: &TestEnv, dry: bool, now: i64) -> CleanupRun {
        c.run(&env.sessions, CleanupScope::default(), dry, now)
            .unwrap()
    }

    fn registered(env: &TestEnv, wt: &Path) -> bool {
        ct_git::find_worktree(&env.repo, wt).unwrap().is_some()
    }

    /// Critério de aceite: worktree de issue com PR mesclado e sem sessão
    /// ativa sai (com o `target/`); o dry-run só relata.
    #[test]
    fn worktree_de_pr_mesclado_sai() {
        let env = env();
        let (wi, wt) = worktree(&env, 9);
        env.sessions
            .ledger()
            .apply_github(&GithubSnapshot {
                project_id: env.project_id,
                issues: vec![],
                prs: vec![PrRow {
                    number: 90,
                    work_item_id: wi,
                    head: "feat/9-x".into(),
                    base: "develop".into(),
                    state: PrState::Merged,
                    url: String::new(),
                    updated_at: 1_000,
                    head_sha: None,
                }],
                etags: vec![],
                synced_at: ct_core::now_ms(),
            })
            .unwrap();
        let c = Cleaner::standard(cfg());
        let now = ct_core::now_ms();
        let plan = c.plan(&env.sessions, now).unwrap();
        let w = step(&plan, wi, StepKind::Worktree);
        assert_eq!(w.state, StepState::Pendente, "{w:?}");
        assert!(w.bytes >= 4096, "{w:?}");
        assert_eq!(w.targets[0], format!("worktree {}", wt.display()));
        // O `target/` vai junto com o worktree.
        assert_eq!(step(&plan, wi, StepKind::Target).state, StepState::Feita);
        // Com o worktree ainda no disco, a branch local (#286) está em uso.
        let b = step(&plan, wi, StepKind::BranchLocal);
        assert_eq!(b.state, StepState::Bloqueada);
        assert!(
            b.reason.as_deref().unwrap().contains("em uso no worktree"),
            "{b:?}"
        );

        let r = run(&c, &env, true, now);
        assert_eq!(r.items[0].steps[0].result, StepResult::Simulada);
        assert_eq!(r.freed_bytes, w.bytes);
        assert!(wt.join("target/debug/deps/lib.rlib").is_file());

        let r = run(&c, &env, false, now);
        let s = &r.items[0].steps[0];
        assert_eq!((s.kind, s.result), (StepKind::Worktree, StepResult::Feita));
        assert_eq!(s.removed, vec![format!("worktree {}", wt.display())]);
        assert!(s.bytes >= 4096);
        assert!(!wt.exists());
        assert!(!registered(&env, &wt));
        let plan = c.plan(&env.sessions, now).unwrap();
        assert_eq!(step(&plan, wi, StepKind::Worktree).state, StepState::Feita);
    }

    /// Critério de aceite: mudança não commitada ou commit não enviado
    /// seguram o worktree; o `target/` sai depois de `target_idle_days`,
    /// e o código fica.
    #[test]
    fn worktree_com_trabalho_fica_e_so_o_target_sai() {
        let env = env();
        let (sujo, wt_sujo) = worktree(&env, 1);
        let (local, wt_local) = worktree(&env, 2);
        fs::write(wt_sujo.join("rascunho.txt"), "x").unwrap();
        fs::write(wt_local.join("b.rs"), "x").unwrap();
        git_in(&wt_local, &["add", "b.rs"]);
        git_in(&wt_local, &["commit", "-q", "-m", "b"]);
        concluir(&env, sujo);
        concluir(&env, local);
        let c = Cleaner::standard(cfg());
        let now = ct_core::now_ms();
        let plan = c.plan(&env.sessions, now).unwrap();
        let w = step(&plan, sujo, StepKind::Worktree);
        assert_eq!(w.state, StepState::Bloqueada);
        assert!(
            w.reason.as_deref().unwrap().contains("rascunho.txt"),
            "{w:?}"
        );
        let w = step(&plan, local, StepKind::Worktree);
        assert_eq!(
            w.reason.as_deref(),
            Some("1 commit(s) não enviado(s) para origin/feat/2-x")
        );
        // Recente: o `target/` também fica.
        let t = step(&plan, sujo, StepKind::Target);
        assert_eq!(t.state, StepState::Bloqueada);
        assert!(
            t.reason.as_deref().unwrap().contains("target_idle_days"),
            "{t:?}"
        );
        run(&c, &env, false, now);
        assert!(wt_sujo.join("target/debug/deps/lib.rlib").is_file());
        assert!(wt_local.join("target/debug/deps/lib.rlib").is_file());

        // Passados os 14 dias: só o `target/` sai, o worktree e o trabalho
        // ficam, e a branch local continua bloqueada (o `held` do worktree
        // a segura; veja `worktree_bloqueado_nao_segura_o_target`).
        let later = now + 15 * DAY;
        let plan = c.plan(&env.sessions, later).unwrap();
        let t = step(&plan, sujo, StepKind::Target);
        assert_eq!(t.state, StepState::Pendente, "{t:?}");
        assert_eq!(
            step(&plan, sujo, StepKind::BranchLocal).state,
            StepState::Bloqueada
        );
        let r = run(&c, &env, true, later);
        assert!(r.freed_bytes >= 2 * 4096);
        assert!(wt_sujo.join("target").is_dir());
        let r = run(&c, &env, false, later);
        for (wt, file) in [(&wt_sujo, "rascunho.txt"), (&wt_local, "b.rs")] {
            assert!(!wt.join("target").exists());
            assert!(wt.join(file).is_file() && wt.join("codigo.rs").is_file());
            assert!(registered(&env, wt));
        }
        assert_eq!(r.count(StepResult::Feita), 2);
        assert!(r.freed_bytes >= 2 * 4096);
    }

    /// Shell livre no worktree (#428) não segura: a remoção o encerra antes,
    /// e o shell de outra pasta continua.
    #[test]
    fn shell_no_worktree_e_encerrado_na_remocao() {
        let env = env();
        let (wi, wt) = worktree(&env, 12);
        let (outro, _) = worktree(&env, 13);
        concluir(&env, wi);
        let sh = env.sessions.open_shell(wi, None).unwrap();
        let fica = env.sessions.open_shell(outro, None).unwrap();
        let c = Cleaner::standard(cfg());
        let later = ct_core::now_ms() + 30 * DAY;
        let plan = c.plan(&env.sessions, later).unwrap();
        let p = step(&plan, wi, StepKind::Worktree);
        assert_eq!(p.state, StepState::Pendente, "{p:?}");
        // O dry-run não encerra nada.
        run(&c, &env, true, later);
        assert!(env.sessions.terminal(&sh.id).is_some());
        run(&c, &env, false, later);
        assert!(!wt.exists());
        env.wait_notice(|n, d| n == "shell_exited" && d["shell_id"] == sh.id.as_str());
        let vivos: Vec<String> = env
            .sessions
            .list_shells()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(vivos, vec![fica.id.clone()]);
        env.sessions.close_shell(&fica.id).unwrap();
    }

    /// Sessão viva no worktree (mesmo de outro work item) segura o worktree
    /// e o `target/`.
    #[test]
    fn sessao_viva_no_worktree_segura() {
        let env = env();
        let (wi, wt) = worktree(&env, 3);
        concluir(&env, wi);
        let outro = env
            .sessions
            .ledger()
            .ensure_work_item(env.project_id, 4, WorkItemKind::Issue)
            .unwrap();
        let s = env
            .sessions
            .ledger()
            .create_session(&NewSession {
                work_item_id: outro.id,
                tool: Tool::Claude,
                cwd: wt.join("sub").to_string_lossy().to_string(),
                launch_cmd: vec!["claude".into()],
                model: None,
                mode: SessionMode::Native,
            })
            .unwrap();
        let c = Cleaner::standard(cfg());
        let later = ct_core::now_ms() + 30 * DAY;
        let plan = c.plan(&env.sessions, later).unwrap();
        for k in [StepKind::Worktree, StepKind::Target] {
            let p = step(&plan, wi, k);
            assert_eq!(p.state, StepState::Bloqueada, "{p:?}");
            assert!(p.reason.as_deref().unwrap().contains(&s.id), "{p:?}");
        }
        run(&c, &env, false, later);
        assert!(wt.join("target").is_dir());
    }

    /// O clone principal nunca sai, nem o `target/` dele; o registro órfão
    /// (pasta apagada à mão) sai no `git worktree prune`.
    #[test]
    fn clone_principal_fica_e_registro_orfao_sai() {
        let env = env();
        let (wi, wt) = worktree(&env, 5);
        concluir(&env, wi);
        fs::remove_dir_all(&wt).unwrap();
        let c = Cleaner::standard(cfg());
        let now = ct_core::now_ms();
        let plan = c.plan(&env.sessions, now).unwrap();
        let w = step(&plan, wi, StepKind::Worktree);
        assert_eq!(w.state, StepState::Pendente, "{w:?}");
        assert!(w.targets[0].contains("registro órfão"), "{w:?}");
        run(&c, &env, false, now);
        assert!(!registered(&env, &wt));
        let plan = c.plan(&env.sessions, now).unwrap();
        assert_eq!(step(&plan, wi, StepKind::Worktree).state, StepState::Feita);

        let l = env.sessions.ledger();
        let main = l
            .ensure_work_item(env.project_id, 6, WorkItemKind::Issue)
            .unwrap();
        l.set_work_item_branch(main.id, "develop", &env.repo.to_string_lossy())
            .unwrap();
        drop(l);
        concluir(&env, main.id);
        fs::create_dir_all(env.repo.join("target")).unwrap();
        let later = now + 30 * DAY;
        let plan = c.plan(&env.sessions, later).unwrap();
        let w = step(&plan, main.id, StepKind::Worktree);
        assert!(
            w.reason.as_deref().unwrap().contains("clone principal"),
            "{w:?}"
        );
        assert_eq!(
            step(&plan, main.id, StepKind::Target).state,
            StepState::Feita
        );
        run(&c, &env, false, later);
        assert!(env.repo.join("target").is_dir());
    }

    /// Critério de aceite: o `target/` de um worktree ainda em andamento,
    /// inativo há `target_idle_days`, sai; o código fica. O dry-run não
    /// apaga nada, e `target_idle_days = 0` desliga.
    #[test]
    fn target_de_worktree_inativo_em_andamento() {
        let env = env();
        let (wi, wt) = worktree(&env, 7);
        let c = Cleaner::standard(cfg());
        let now = ct_core::now_ms();
        // Ativo: fora do plano.
        assert!(c.plan(&env.sessions, now).unwrap().items.is_empty());

        let later = now + 15 * DAY;
        let plan = c.plan(&env.sessions, later).unwrap();
        assert_eq!(plan.items.len(), 1, "{plan:?}");
        let it = &plan.items[0];
        assert_eq!((it.work_item_id, it.completion), (wi, None));
        assert!(it.last_activity_at.is_some());
        assert_eq!(it.steps.len(), 1);
        let t = &it.steps[0];
        assert_eq!((t.kind, t.state), (StepKind::Target, StepState::Pendente));
        assert_eq!(
            t.targets,
            vec![format!("{} (o código fica)", wt.join("target").display())]
        );
        assert_eq!((plan.pending_items, plan.pending_bytes), (1, t.bytes));

        let r = run(&c, &env, true, later);
        assert_eq!(r.items[0].steps[0].result, StepResult::Simulada);
        assert!(wt.join("target/debug/deps/lib.rlib").is_file());

        let off = Cleaner::standard(CleanupConfig {
            target_idle_days: 0,
            ..cfg()
        });
        assert!(off.plan(&env.sessions, later).unwrap().items.is_empty());
        run(&off, &env, false, later);
        assert!(wt.join("target").is_dir());

        let r = run(&c, &env, false, later);
        let s = &r.items[0].steps[0];
        assert_eq!((s.kind, s.result), (StepKind::Target, StepResult::Feita));
        assert_eq!(s.bytes, t.bytes);
        assert!(!wt.join("target").exists());
        assert!(wt.join("codigo.rs").is_file());
        assert!(registered(&env, &wt));
        assert!(c.plan(&env.sessions, later).unwrap().items.is_empty());
        // O work item continua em andamento.
        assert_eq!(
            env.sessions.ledger().get_work_item(wi).unwrap().status,
            WorkItemStatus::Backlog
        );
    }
}
