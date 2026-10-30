//! Etapas "branch local" e "branch remota" da limpeza (issue #286, épico
//! #284). Rodam por último, depois do worktree (#265): com o worktree
//! `feita`, ele já saiu do disco e do `git worktree list`, e a branch não
//! está mais em uso.
//!
//! [`BranchLocalStep`] apaga `wi.branch` no clone principal quando:
//! - o último PR da branch foi mesclado e a ponta local é o head do PR (ou
//!   está contida nele). Cobre o squash merge, em que o `git branch -d`
//!   recusa por não achar os commits na base;
//! - ou o PR foi fechado sem merge (ou não houve PR, e a issue fechou) e
//!   a branch não tem commit fora do remoto.
//!
//! A remoção é condicionada ao SHA avaliado (`git update-ref -d` com o valor
//! antigo, [`ct_git::delete_branch`]): um commit feito entre o plano e a
//! execução segura a branch.
//!
//! [`BranchRemoteStep`] apaga `origin/<branch>` na forja quando o último PR
//! foi mesclado ou fechado e a ponta remota é o head do PR: ninguém empurrou
//! depois. O plano usa a referência remota local
//! (`refs/remotes/origin/<branch>`, sem rede); a execução confere a ponta na
//! forja na hora, apaga e roda `git fetch --prune`. A forja fica atrás de
//! [`RemoteBranches`]: o GitHub pela API, com o token do daemon
//! ([`GithubSync`]), e o GitLab pelo glab, com o comando de credencial do
//! projeto ([`GitlabRemote`], #350). No GitLab, o PR é o MR, e as mensagens
//! dizem `MR !<iid>`.
//!
//! O head do PR vem do espelho (`pr_cache.head_sha`, gravado pelo sync: a
//! listagem com ETag do GitHub ou o `sha` do `glab mr list`); o PR gravado
//! antes da coluna é buscado uma vez na forja
//! ([`RemoteBranches::pr_head_sha`]).
//!
//! Nunca saem ([`never_touch`]), nem a local nem a remota: `develop`,
//! `main`, `master`, a base do projeto, as protegidas do projeto,
//! `release-please--*`, a branch que é head ou base de um PR (MR) aberto e a de
//! épico com sub-issue aberta (o work item dono da branch registrada, sem
//! supor o nome `wip/<n>-…`). O que fica aparece como
//! "<branch> não removida: <motivo>".
//!
//! [`GithubSync`]: crate::github::GithubSync
//! [`GitlabRemote`]: crate::gitlab::GitlabRemote

use crate::cleanup::{CleanupStep, CleanupTarget, StepContext, StepKind, StepOutcome, StepPlan};
use crate::github::GithubSync;
use crate::{Result, SessionsError};
use ct_core::{change_label, change_ref, ForgeKind, PrState};
use ct_ledger::{Ledger, PrRow, Project};
use std::path::Path;

/// A branch remota e o head do PR (MR) na forja, para a etapa "branch
/// remota" (#286, #350). O GitHub e o GitLab implementam.
pub trait RemoteBranches: Send + Sync {
    /// Head do PR que entrou no espelho sem ele, buscado na forja (e
    /// gravado). `None` = o PR não existe ou não há acesso.
    fn pr_head_sha(&self, project: &Project, pr: &PrRow) -> Result<Option<String>>;
    /// Ponta da branch na forja agora; `None` = ela não existe.
    fn remote_branch_sha(&self, project: &Project, branch: &str) -> Result<Option<String>>;
    /// Apaga a branch na forja; `false` = já não existia.
    fn delete_remote_branch(&self, project: &Project, branch: &str) -> Result<bool>;
}

impl RemoteBranches for GithubSync {
    fn pr_head_sha(&self, project: &Project, pr: &PrRow) -> Result<Option<String>> {
        GithubSync::pr_head_sha(self, project, pr)
    }

    fn remote_branch_sha(&self, project: &Project, branch: &str) -> Result<Option<String>> {
        GithubSync::remote_branch_sha(self, project, branch)
    }

    fn delete_remote_branch(&self, project: &Project, branch: &str) -> Result<bool> {
        GithubSync::delete_remote_branch(self, project, branch)
    }
}

fn forge_of(ctx: &StepContext<'_>) -> ForgeKind {
    ctx.target.project.forge.kind()
}

/// O remoto das branches de trabalho.
pub const REMOTE: &str = "origin";

/// Branches padrão que nunca saem, qualquer que seja a configuração.
const ALWAYS_KEPT: [&str; 3] = ["develop", "main", "master"];

fn short(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

fn branch_of(t: &CleanupTarget) -> Option<&str> {
    t.work_item
        .branch
        .as_deref()
        .filter(|b| !b.trim().is_empty())
}

/// O clone principal, se existir.
fn repo_of(t: &CleanupTarget) -> Option<&Path> {
    let p = Path::new(&t.project.repo_path);
    (!t.project.repo_path.trim().is_empty() && p.is_dir()).then_some(p)
}

/// Número do épico de uma branch `wip/<n>` ou `wip/<n>-…` (o padrão do
/// GitHub; vale como reserva para a branch que não foi registrada no work
/// item, como a de épicos antigos ou abertos fora do Control Tower).
fn epic_number(branch: &str) -> Option<i64> {
    let rest = branch.strip_prefix("wip/")?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    let (n, tail) = rest.split_at(end);
    if n.is_empty() || !(tail.is_empty() || tail.starts_with('-')) {
        return None;
    }
    n.parse().ok()
}

/// Por que a branch nunca sai (local nem remota), se for o caso: padrão ou
/// base, protegida, do release-please, head ou base de PR aberto, ou de
/// épico com sub-issue aberta.
pub fn never_touch(l: &Ledger, project: &Project, branch: &str) -> Result<Option<String>> {
    if ALWAYS_KEPT.contains(&branch) || branch == project.base_branch {
        return Ok(Some("branch padrão ou base".into()));
    }
    if project.protected_branches.iter().any(|b| b == branch) {
        return Ok(Some("branch protegida".into()));
    }
    if branch.starts_with("release-please--") {
        return Ok(Some("branch do release-please".into()));
    }
    let forge = project.forge.kind();
    let (label, short) = (change_label(forge), |n| ct_core::change_short(forge, n));
    for pr in l.open_prs(project.id)? {
        if pr.head == branch {
            return Ok(Some(format!("head do {label} aberto {}", short(pr.number))));
        }
        if pr.base == branch {
            return Ok(Some(format!("base do {label} aberto {}", short(pr.number))));
        }
    }
    // O épico é achado pela branch registrada no work item (spec do Jira e
    // GitLab §4): o padrão do nome varia por projeto (`PROJ-5450` no exemplo).
    // No GitHub, o nome `wip/<n>-…` continua valendo para a branch não
    // registrada.
    let mut owners: Vec<i64> = l
        .work_items_by_branch(project.id, branch)?
        .into_iter()
        .map(|w| w.gh_number)
        .collect();
    if project.tracker.kind() == ct_core::TrackerKind::Github {
        owners.extend(epic_number(branch).filter(|n| !owners.contains(n)));
    }
    if !owners.is_empty() {
        let t = project.tracker_ref();
        let open: Vec<String> = l
            .list_issues(project.id)?
            .into_iter()
            .filter(|i| i.state == "open" && i.parent_number.is_some_and(|p| owners.contains(&p)))
            .map(|i| ct_core::item_short(&t, i.gh_number))
            .collect();
        if !open.is_empty() {
            return Ok(Some(format!(
                "branch de épico com sub-issue aberta ({})",
                open.join(", ")
            )));
        }
    }
    Ok(None)
}

/// Motivo de nunca tocar na branch e o último PR dela, lidos de uma vez
/// (lock curto no ledger).
fn ledger_facts(ctx: &StepContext<'_>, branch: &str) -> Result<(Option<String>, Option<PrRow>)> {
    let l = ctx.sessions.ledger();
    let never = never_touch(&l, &ctx.target.project, branch)?;
    // Do mais recente para o mais antigo: vale o último PR da branch.
    let pr = l
        .prs_for_work_item(ctx.target.work_item.id)?
        .into_iter()
        .find(|p| p.head == branch);
    Ok((never, pr))
}

/// Head do PR, ou o motivo de não saber.
fn pr_head(ctx: &StepContext<'_>, pr: &PrRow) -> Result<std::result::Result<String, String>> {
    if let Some(s) = &pr.head_sha {
        return Ok(Ok(s.clone()));
    }
    let forge = forge_of(ctx);
    let Some(g) = ctx.remote() else {
        return Ok(Err(format!(
            "head do {} desconhecido (sync com o {} desativado)",
            change_ref(forge, pr.number),
            forge.label()
        )));
    };
    Ok(g.pr_head_sha(&ctx.target.project, pr)?.ok_or_else(|| {
        format!(
            "{} não encontrado no {}",
            change_ref(forge, pr.number),
            forge.label()
        )
    }))
}

fn pr_label(forge: ForgeKind, pr: &PrRow) -> String {
    let estado = if pr.state == PrState::Merged {
        "mesclado"
    } else {
        "fechado"
    };
    format!("{} {estado}", change_ref(forge, pr.number))
}

/// Avaliação de uma etapa de branch.
enum Eval {
    Feita,
    Bloqueada(String),
    /// `sha`: a ponta avaliada (local) ou o head do PR (remota).
    Pendente {
        sha: String,
        target: String,
    },
}

impl Eval {
    fn plan(self, kind: StepKind) -> StepPlan {
        match self {
            Eval::Feita => StepPlan::feita(kind),
            Eval::Bloqueada(r) => StepPlan::bloqueada(kind, r),
            Eval::Pendente { target, .. } => StepPlan::pendente(kind, 0, vec![target]),
        }
    }
}

/// Etapa "branch local" (#286); veja o topo do módulo.
pub struct BranchLocalStep;

impl BranchLocalStep {
    fn eval(ctx: &StepContext<'_>) -> Result<Eval> {
        let (Some(branch), Some(repo)) = (branch_of(ctx.target), repo_of(ctx.target)) else {
            return Ok(Eval::Feita);
        };
        let Some(tip) = ct_git::branch_sha(repo, branch)? else {
            return Ok(Eval::Feita);
        };
        let kept = |why: String| Ok(Eval::Bloqueada(format!("{branch} não removida: {why}")));
        let forge = forge_of(ctx);
        let pr_label = |pr: &PrRow| pr_label(forge, pr);
        let (never, pr) = ledger_facts(ctx, branch)?;
        if let Some(r) = never {
            return kept(r);
        }
        if let Some(wt) = ct_git::worktree_with_branch(repo, branch)? {
            return kept(format!("em uso no worktree {}", wt.path.display()));
        }
        let why = match pr {
            Some(pr) if pr.state == PrState::Merged => {
                let head = match pr_head(ctx, &pr)? {
                    Ok(h) => h,
                    Err(r) => return kept(r),
                };
                if tip == head {
                    format!("{}, igual ao head do {}", short(&tip), pr_label(&pr))
                } else if ct_git::is_ancestor(repo, &tip, &head)? {
                    format!("{}, contida no head do {}", short(&tip), pr_label(&pr))
                } else {
                    return kept(format!(
                        "ponta local {} difere do head do {} ({}): commit(s) depois do {}",
                        short(&tip),
                        pr_label(&pr),
                        short(&head),
                        change_label(forge)
                    ));
                }
            }
            pr => {
                let n = ct_git::commits_not_on_remotes(repo, &tip)?;
                if n > 0 {
                    return kept(format!("{n} commit(s) que não estão no remoto"));
                }
                let sinal = match &pr {
                    Some(pr) => pr_label(pr),
                    None => ctx
                        .target
                        .completion
                        .map(|c| c.signal.label_for(forge))
                        .unwrap_or_else(|| "concluída".into()),
                };
                format!("{}, todos os commits no remoto; {sinal}", short(&tip))
            }
        };
        Ok(Eval::Pendente {
            sha: tip,
            target: format!("branch local {branch} ({why})"),
        })
    }
}

impl CleanupStep for BranchLocalStep {
    fn kind(&self) -> StepKind {
        StepKind::BranchLocal
    }

    fn plan(&self, ctx: &StepContext<'_>) -> Result<StepPlan> {
        Ok(Self::eval(ctx)?.plan(self.kind()))
    }

    fn run(&self, ctx: &StepContext<'_>) -> Result<StepOutcome> {
        // Reavalia na hora: nada mudou desde o plano?
        let (sha, target) = match Self::eval(ctx)? {
            Eval::Pendente { sha, target } => (sha, target),
            Eval::Bloqueada(r) => return Err(SessionsError::Invalid(r)),
            Eval::Feita => {
                return Err(SessionsError::Invalid(
                    "a branch local já não existe".into(),
                ))
            }
        };
        let branch = branch_of(ctx.target).expect("plano pendente tem branch");
        let repo = repo_of(ctx.target).expect("plano pendente tem clone");
        ct_git::delete_branch(repo, branch, &sha)?;
        tracing::info!("limpeza: {target} apagada");
        Ok(StepOutcome {
            freed_bytes: 0,
            removed: vec![target],
            kept: Vec::new(),
        })
    }
}

/// Etapa "branch remota" (#286); veja o topo do módulo.
pub struct BranchRemoteStep;

impl BranchRemoteStep {
    fn eval(ctx: &StepContext<'_>) -> Result<Eval> {
        let (Some(branch), Some(repo)) = (branch_of(ctx.target), repo_of(ctx.target)) else {
            return Ok(Eval::Feita);
        };
        let Some(remote) = ct_git::remote_branch_sha(repo, REMOTE, branch)? else {
            return Ok(Eval::Feita);
        };
        let kept = |why: String| {
            Ok(Eval::Bloqueada(format!(
                "{REMOTE}/{branch} não removida: {why}"
            )))
        };
        let forge = forge_of(ctx);
        let label = change_label(forge);
        let pr_label = |pr: &PrRow| pr_label(forge, pr);
        let (never, pr) = ledger_facts(ctx, branch)?;
        if let Some(r) = never {
            return kept(r);
        }
        let Some(pr) = pr else {
            return kept(format!(
                "sem {label} da branch; a remota só sai com o {label} mesclado ou fechado"
            ));
        };
        if pr.state == PrState::Open {
            return kept(format!("{} aberto", change_ref(forge, pr.number)));
        }
        if ctx.remote().is_none() {
            return kept(match forge {
                ForgeKind::Github => "sync com o GitHub desativado (sem a API para apagar)".into(),
                ForgeKind::Gitlab => "glab desativado (sem o GitLab para apagar)".into(),
            });
        }
        let head = match pr_head(ctx, &pr)? {
            Ok(h) => h,
            Err(r) => return kept(r),
        };
        if remote != head {
            return kept(format!(
                "ponta remota {} difere do head do {} ({}): houve push depois do {label}",
                short(&remote),
                pr_label(&pr),
                short(&head)
            ));
        }
        Ok(Eval::Pendente {
            target: format!(
                "branch remota {REMOTE}/{branch} ({}, igual ao head do {})",
                short(&head),
                pr_label(&pr)
            ),
            sha: head,
        })
    }
}

impl CleanupStep for BranchRemoteStep {
    fn kind(&self) -> StepKind {
        StepKind::BranchRemota
    }

    fn plan(&self, ctx: &StepContext<'_>) -> Result<StepPlan> {
        Ok(Self::eval(ctx)?.plan(self.kind()))
    }

    fn run(&self, ctx: &StepContext<'_>) -> Result<StepOutcome> {
        let (head, target) = match Self::eval(ctx)? {
            Eval::Pendente { sha, target } => (sha, target),
            Eval::Bloqueada(r) => return Err(SessionsError::Invalid(r)),
            Eval::Feita => {
                return Err(SessionsError::Invalid(
                    "a branch remota já não existe".into(),
                ))
            }
        };
        let branch = branch_of(ctx.target).expect("plano pendente tem branch");
        let repo = repo_of(ctx.target).expect("plano pendente tem clone");
        let g = ctx.remote().expect("plano pendente tem a forja");
        let forge = forge_of(ctx);
        let (name, label) = (forge.label(), change_label(forge));
        let project = &ctx.target.project;
        let mut o = StepOutcome::default();
        // A referência local pode estar atrasada: vale a ponta na forja agora.
        match g.remote_branch_sha(project, branch)? {
            None => o.removed.push(format!(
                "{REMOTE}/{branch} já não existia no {name} (referência local podada)"
            )),
            Some(now) if now != head => {
                return Err(SessionsError::Invalid(format!(
                    "{REMOTE}/{branch} não removida: no {name} está em {}, depois do head do \
                     {label} ({}); nada foi apagado",
                    short(&now),
                    short(&head)
                )))
            }
            Some(_) => {
                g.delete_remote_branch(project, branch)?;
                tracing::info!("limpeza: {target} apagada no {name}");
                o.removed.push(target);
            }
        }
        if let Err(e) = ct_git::fetch_prune(repo, REMOTE) {
            o.kept
                .push(format!("git fetch --prune {REMOTE} falhou: {e}"));
        }
        Ok(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cleanup::{Cleaner, CleanupPlan, CleanupRun, CleanupScope, StepResult, StepState};
    use crate::config::CleanupConfig;
    use crate::github::GithubSync;
    use crate::test_support::{git_in, FakeClaude, TestEnv};
    use ct_core::{WorkItemKind, WorkItemStatus};
    use ct_gh::fake::{fake_gh, FakeGitHub};
    use ct_gh::refs::{branch_ref_path, branch_refs_path, pull_path};
    use ct_gh::{GhClient, GhConfig};
    use ct_ledger::{GithubSnapshot, IssueRow};
    use serde_json::json;
    use std::sync::Arc;

    const OWNER: &str = "deep-ion-ai";
    const NAME: &str = "demo";

    fn cfg() -> CleanupConfig {
        CleanupConfig {
            min_age_hours: 0,
            min_idle_hours: 0,
            ..CleanupConfig::default()
        }
    }

    struct Env {
        env: TestEnv,
        gh: FakeGitHub,
        github: Arc<GithubSync>,
        origin: std::path::PathBuf,
    }

    /// `TestEnv` com `origin` (bare) e um GitHub falso.
    fn setup() -> Env {
        let env = TestEnv::new(FakeClaude::exit(0));
        let origin = env.tmp.path().join("origin.git");
        let o = origin.to_string_lossy().to_string();
        git_in(env.tmp.path(), &["init", "-q", "--bare", &o]);
        git_in(&env.repo, &["remote", "add", "origin", &o]);
        git_in(&env.repo, &["push", "-q", "-u", "origin", "develop"]);
        let gh = FakeGitHub::start("tok");
        let bin = fake_gh(env.tmp.path(), "tok");
        let client = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT),
        );
        let github = Arc::new(GithubSync::new(env.sessions.clone(), client));
        Env {
            env,
            gh,
            github,
            origin,
        }
    }

    impl Env {
        fn repo(&self) -> &Path {
            &self.env.repo
        }

        fn cleaner(&self) -> Cleaner {
            Cleaner::standard(cfg()).with_github(Some(Arc::clone(&self.github)))
        }

        /// Work item `n` concluído com a branch `branch` (sem worktree): um
        /// commit enviado para `origin`. Devolve (work item, ponta).
        fn branch(&self, n: i64, branch: &str) -> (i64, String) {
            git_in(self.repo(), &["checkout", "-q", "-b", branch]);
            git_in(
                self.repo(),
                &["commit", "-q", "--allow-empty", "-m", &format!("issue {n}")],
            );
            git_in(self.repo(), &["push", "-q", "-u", "origin", branch]);
            git_in(self.repo(), &["checkout", "-q", "develop"]);
            let l = self.env.sessions.ledger();
            let wi = l
                .ensure_work_item(self.env.project_id, n, WorkItemKind::Issue)
                .unwrap();
            l.set_work_item_branch(wi.id, branch, "").unwrap();
            l.set_work_item_status(wi.id, WorkItemStatus::Concluido, "teste")
                .unwrap();
            drop(l);
            (wi.id, self.tip(branch).unwrap())
        }

        /// Commit novo na branch, sem checkout (`commit-tree` + `update-ref`).
        fn commit_on(&self, branch: &str) -> String {
            let parent = self.tip(branch).unwrap();
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(self.repo())
                .args(["commit-tree", &format!("{parent}^{{tree}}"), "-p", &parent])
                .args(["-m", "depois do PR"])
                .output()
                .unwrap();
            assert!(out.status.success());
            let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
            git_in(
                self.repo(),
                &["update-ref", &format!("refs/heads/{branch}"), &sha],
            );
            sha
        }

        fn tip(&self, branch: &str) -> Option<String> {
            ct_git::branch_sha(self.repo(), branch).unwrap()
        }

        fn remote(&self, branch: &str) -> Option<String> {
            ct_git::remote_branch_sha(self.repo(), "origin", branch).unwrap()
        }

        /// PR `number` da branch no espelho, com o head `head`.
        fn pr(&self, number: i64, wi: i64, branch: &str, state: PrState, head: Option<&str>) {
            self.env
                .sessions
                .ledger()
                .apply_github(&GithubSnapshot {
                    project_id: self.env.project_id,
                    issues: vec![],
                    prs: vec![PrRow {
                        number,
                        work_item_id: wi,
                        head: branch.into(),
                        base: "develop".into(),
                        state,
                        url: String::new(),
                        updated_at: 1_000,
                        head_sha: head.map(String::from),
                    }],
                    etags: vec![],
                    synced_at: ct_core::now_ms(),
                })
                .unwrap();
        }

        /// A branch publicada no GitHub falso, na ponta `sha`.
        fn publish(&self, branch: &str, sha: &str) {
            self.gh.set(
                &branch_ref_path(OWNER, NAME, branch),
                json!({ "ref": format!("refs/heads/{branch}"), "object": { "sha": sha } }),
            );
        }

        fn deletes(&self) -> Vec<String> {
            self.gh
                .calls()
                .into_iter()
                .filter(|(m, _)| m == "DELETE")
                .map(|(_, p)| p)
                .collect()
        }

        fn plan(&self, c: &Cleaner) -> CleanupPlan {
            c.plan(&self.env.sessions, ct_core::now_ms()).unwrap()
        }

        fn run(&self, c: &Cleaner, dry: bool) -> CleanupRun {
            c.run(
                &self.env.sessions,
                CleanupScope::default(),
                dry,
                ct_core::now_ms(),
            )
            .unwrap()
        }
    }

    fn step(plan: &CleanupPlan, wi: i64, kind: StepKind) -> StepPlan {
        plan.items
            .iter()
            .find(|i| i.work_item_id == wi)
            .and_then(|i| i.steps.iter().find(|s| s.kind == kind))
            .cloned()
            .unwrap_or_else(|| panic!("sem a etapa {kind} do work item {wi}: {plan:?}"))
    }

    fn reason(p: &StepPlan) -> &str {
        p.reason.as_deref().unwrap_or_default()
    }

    /// Critério de aceite: branch local e remota de PR mesclado por squash
    /// (os commits não estão em `develop`) saem; o dry-run só lista.
    #[test]
    fn pr_mesclado_por_squash_sai_local_e_remota() {
        let e = setup();
        let (wi, tip) = e.branch(9, "feat/9-x");
        e.pr(90, wi, "feat/9-x", PrState::Merged, Some(&tip));
        e.publish("feat/9-x", &tip);
        let c = e.cleaner();
        let plan = e.plan(&c);
        let local = step(&plan, wi, StepKind::BranchLocal);
        assert_eq!(local.state, StepState::Pendente, "{local:?}");
        assert_eq!(
            local.targets,
            vec![format!(
                "branch local feat/9-x ({}, igual ao head do PR #90 mesclado)",
                &tip[..7]
            )]
        );
        let remota = step(&plan, wi, StepKind::BranchRemota);
        assert_eq!(remota.state, StepState::Pendente, "{remota:?}");
        assert!(remota.targets[0].starts_with("branch remota origin/feat/9-x"));
        assert_eq!(plan.pending_items, 1);

        let r = e.run(&c, true);
        let res: Vec<_> = r.items[0]
            .steps
            .iter()
            .map(|s| (s.kind, s.result))
            .collect();
        assert_eq!(
            res,
            [
                (StepKind::BranchLocal, StepResult::Simulada),
                (StepKind::BranchRemota, StepResult::Simulada)
            ]
        );
        assert_eq!(e.tip("feat/9-x"), Some(tip.clone()));
        assert!(e.deletes().is_empty());

        // O DELETE do GitHub falso não chega ao `origin` local: apaga-se à
        // mão, como o GitHub faria, para o `fetch --prune` ter o que podar.
        git_in(&e.origin, &["branch", "-D", "feat/9-x"]);
        let r = e.run(&c, false);
        let s = &r.items[0].steps;
        assert_eq!(
            (s[0].kind, s[0].result),
            (StepKind::BranchLocal, StepResult::Feita),
            "{r:?}"
        );
        assert_eq!(s[0].removed, local.targets);
        assert_eq!(
            (s[1].kind, s[1].result),
            (StepKind::BranchRemota, StepResult::Feita),
            "{r:?}"
        );
        assert!(s[1].kept.is_empty(), "{r:?}");
        assert_eq!(e.tip("feat/9-x"), None);
        assert_eq!(e.deletes(), vec![branch_refs_path(OWNER, NAME, "feat/9-x")]);
        // O `git fetch --prune` tirou a referência remota.
        assert_eq!(e.remote("feat/9-x"), None);
        assert!(e.plan(&c).items.is_empty());
    }

    /// Critério de aceite: commit local depois do head do PR segura a
    /// branch local (e a remota espera); push depois do PR segura a remota.
    #[test]
    fn commit_ou_push_depois_do_pr_nunca_sai() {
        let e = setup();
        let (wi, head) = e.branch(9, "feat/9-x");
        e.pr(90, wi, "feat/9-x", PrState::Merged, Some(&head));
        e.publish("feat/9-x", &head);
        let depois = e.commit_on("feat/9-x");
        let c = e.cleaner();
        let plan = e.plan(&c);
        let local = step(&plan, wi, StepKind::BranchLocal);
        assert_eq!(local.state, StepState::Bloqueada);
        assert!(
            reason(&local).starts_with("feat/9-x não removida: ponta local")
                && reason(&local).contains("commit(s) depois do PR"),
            "{local:?}"
        );
        let remota = step(&plan, wi, StepKind::BranchRemota);
        assert_eq!(reason(&remota), "aguarda a etapa branch local");
        e.run(&c, false);
        assert_eq!(e.tip("feat/9-x"), Some(depois.clone()));
        assert!(e.deletes().is_empty());

        // O commit foi enviado: a local volta ao head do PR e sai, mas a
        // remota tem um push depois do PR e fica.
        git_in(e.repo(), &["push", "-q", "origin", "feat/9-x"]);
        git_in(e.repo(), &["update-ref", "refs/heads/feat/9-x", &head]);
        let plan = e.plan(&c);
        assert_eq!(
            step(&plan, wi, StepKind::BranchLocal).state,
            StepState::Pendente
        );
        let remota = step(&plan, wi, StepKind::BranchRemota);
        assert_eq!(remota.state, StepState::Bloqueada);
        assert!(
            reason(&remota).starts_with("origin/feat/9-x não removida: ponta remota")
                && reason(&remota).contains("houve push depois do PR"),
            "{remota:?}"
        );
        let r = e.run(&c, false);
        assert_eq!(r.items[0].steps[0].result, StepResult::Feita);
        assert_eq!(r.items[0].steps[1].result, StepResult::Bloqueada);
        assert_eq!(e.tip("feat/9-x"), None);
        assert_eq!(e.remote("feat/9-x"), Some(depois));
        assert!(e.deletes().is_empty());
    }

    /// A referência local diz que a remota está no head do PR, mas no
    /// GitHub alguém empurrou depois: a execução confere e não apaga.
    #[test]
    fn execucao_confere_a_ponta_no_github() {
        let e = setup();
        let (wi, head) = e.branch(9, "feat/9-x");
        e.pr(90, wi, "feat/9-x", PrState::Closed, Some(&head));
        e.publish("feat/9-x", "f00f00f00f00f00f00f00f00f00f00f00f00f00f");
        let c = e.cleaner();
        let plan = e.plan(&c);
        assert_eq!(
            step(&plan, wi, StepKind::BranchRemota).state,
            StepState::Pendente
        );
        let r = e.run(&c, false);
        let s = &r.items[0].steps[1];
        assert_eq!(
            (s.kind, s.result),
            (StepKind::BranchRemota, StepResult::Falhou)
        );
        assert!(
            s.reason.as_deref().unwrap().contains("nada foi apagado"),
            "{s:?}"
        );
        assert!(e.deletes().is_empty());
        assert!(e.remote("feat/9-x").is_some());
    }

    /// Critério de aceite: PR fechado sem merge com commit não enviado
    /// fica; sem PR, a remota nunca sai.
    #[test]
    fn fechado_sem_merge_so_sai_sem_commit_fora_do_remoto() {
        let e = setup();
        let (wi, head) = e.branch(9, "feat/9-x");
        e.pr(90, wi, "feat/9-x", PrState::Closed, Some(&head));
        e.commit_on("feat/9-x");
        let c = e.cleaner();
        let local = step(&e.plan(&c), wi, StepKind::BranchLocal);
        assert_eq!(
            reason(&local),
            "feat/9-x não removida: 1 commit(s) que não estão no remoto"
        );
        e.run(&c, false);
        assert!(e.tip("feat/9-x").is_some());

        // Sem PR (issue concluída): a local sem commit fora do remoto sai;
        // a remota fica, com o motivo.
        let (w2, _) = e.branch(10, "feat/10-y");
        let plan = e.plan(&c);
        let local = step(&plan, w2, StepKind::BranchLocal);
        assert_eq!(local.state, StepState::Pendente, "{local:?}");
        assert!(local.targets[0].contains("todos os commits no remoto"));
        let remota = step(&plan, w2, StepKind::BranchRemota);
        assert_eq!(
            reason(&remota),
            "origin/feat/10-y não removida: sem PR da branch; a remota só sai com o PR \
             mesclado ou fechado"
        );
        e.run(&c, false);
        assert_eq!(e.tip("feat/10-y"), None);
        assert!(e.remote("feat/10-y").is_some());
    }

    /// PR gravado antes da coluna `head_sha`: com o GitHub, o head é buscado
    /// uma vez e gravado; sem ele, a branch fica com o motivo.
    #[test]
    fn head_do_pr_antigo_buscado_sob_demanda() {
        let e = setup();
        let (wi, tip) = e.branch(9, "feat/9-x");
        e.pr(90, wi, "feat/9-x", PrState::Merged, None);
        let sem_gh = Cleaner::standard(cfg());
        let plan = e.plan(&sem_gh);
        assert_eq!(
            reason(&step(&plan, wi, StepKind::BranchLocal)),
            "feat/9-x não removida: head do PR #90 desconhecido (sync com o GitHub desativado)"
        );
        // A remota espera a local.
        assert_eq!(
            step(&plan, wi, StepKind::BranchRemota).state,
            StepState::Bloqueada
        );

        e.gh.set(
            &pull_path(OWNER, NAME, 90),
            json!({ "number": 90, "head": { "ref": "feat/9-x", "sha": tip } }),
        );
        let plan = e.plan(&e.cleaner());
        assert_eq!(
            step(&plan, wi, StepKind::BranchLocal).state,
            StepState::Pendente
        );
        let prs = e.env.sessions.ledger().prs_for_work_item(wi).unwrap();
        assert_eq!(prs[0].head_sha.as_deref(), Some(tip.as_str()));
        // Já gravado: não busca de novo.
        e.gh.clear_requests();
        e.plan(&e.cleaner());
        assert!(e.gh.calls().iter().all(|(_, p)| !p.contains("/pulls/")));
    }

    /// Critério de aceite: padrão, base, protegidas, `release-please--*`,
    /// head ou base de PR aberto e épico com sub-issue aberta nunca saem.
    #[test]
    fn branches_preservadas() {
        let e = setup();
        let mut project = e
            .env
            .sessions
            .ledger()
            .get_project(e.env.project_id)
            .unwrap();
        project.base_branch = "trunk".into();
        project.protected_branches = vec!["estavel".into()];
        // PR aberto do work item 20: head `feat/20-a`, base `wip/30-epico`.
        let w20 = e
            .env
            .sessions
            .ledger()
            .ensure_work_item(e.env.project_id, 20, WorkItemKind::Issue)
            .unwrap()
            .id;
        let mut aberto = PrRow {
            number: 200,
            work_item_id: w20,
            head: "feat/20-a".into(),
            base: "wip/30-epico".into(),
            state: PrState::Open,
            url: String::new(),
            updated_at: 1,
            head_sha: None,
        };
        let issue = |n: i64, state: &str, parent: Option<i64>| IssueRow {
            gh_number: n,
            title: format!("Issue {n}"),
            state: state.into(),
            labels: vec![],
            parent_number: parent,
            has_sub_issues: false,
            url: String::new(),
            updated_at: 1,
            sprints: Vec::new(),
        };
        let snap = |prs: Vec<PrRow>, issues: Vec<IssueRow>| {
            e.env
                .sessions
                .ledger()
                .apply_github(&GithubSnapshot {
                    project_id: e.env.project_id,
                    issues,
                    prs,
                    etags: vec![],
                    synced_at: 1,
                })
                .unwrap();
        };
        // Épicos com a branch registrada no work item: `wip/40-outro` e, num
        // padrão sem `wip/`, `EPICO-50`.
        for (n, b) in [(40, "wip/40-outro"), (50, "EPICO-50")] {
            let l = e.env.sessions.ledger();
            let wi = l
                .ensure_work_item(e.env.project_id, n, WorkItemKind::Epic)
                .unwrap();
            l.set_work_item_branch(wi.id, b, "").unwrap();
        }
        snap(
            vec![aberto.clone()],
            vec![
                issue(41, "open", Some(40)),
                issue(42, "closed", Some(40)),
                issue(51, "open", Some(50)),
            ],
        );
        let never = |b: &str| never_touch(&e.env.sessions.ledger(), &project, b).unwrap();
        for b in ["develop", "main", "master", "trunk"] {
            assert_eq!(never(b).as_deref(), Some("branch padrão ou base"), "{b}");
        }
        assert_eq!(never("estavel").as_deref(), Some("branch protegida"));
        assert_eq!(
            never("release-please--branches--main").as_deref(),
            Some("branch do release-please")
        );
        assert_eq!(
            never("feat/20-a").as_deref(),
            Some("head do PR aberto #200")
        );
        assert_eq!(
            never("wip/30-epico").as_deref(),
            Some("base do PR aberto #200")
        );
        assert_eq!(
            never("wip/40-outro").as_deref(),
            Some("branch de épico com sub-issue aberta (#41)")
        );
        assert_eq!(
            never("EPICO-50").as_deref(),
            Some("branch de épico com sub-issue aberta (#51)")
        );
        // No GitHub, o nome `wip/<n>` sem registro continua protegendo.
        assert_eq!(
            never("wip/40-outra").as_deref(),
            Some("branch de épico com sub-issue aberta (#41)")
        );
        assert_eq!(never("wip/400-outro"), None);
        assert_eq!(never("wip/40x"), None);
        assert_eq!(never("feat/9-x"), None);
        // PR mesclado e sub-issues fechadas: deixam de segurar.
        aberto.state = PrState::Merged;
        snap(
            vec![aberto],
            vec![issue(41, "closed", Some(40)), issue(51, "closed", Some(50))],
        );
        assert_eq!(never("feat/20-a"), None);
        assert_eq!(never("wip/30-epico"), None);
        assert_eq!(never("wip/40-outro"), None);
        assert_eq!(never("EPICO-50"), None);

        // Projeto Jira: o nome `wip/<n>` sem registro não faz épico.
        let mut jira = project.clone();
        jira.tracker = ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
            project_key: "P".into(),
            ..Default::default()
        });
        snap(vec![], vec![issue(41, "open", Some(40))]);
        let never_jira = |b: &str| never_touch(&e.env.sessions.ledger(), &jira, b).unwrap();
        assert_eq!(never_jira("wip/40-outra"), None);
        assert_eq!(
            never_jira("wip/40-outro").as_deref(),
            Some("branch de épico com sub-issue aberta (P-41)")
        );
    }

    /// Pela etapa: a branch de épico concluído com sub-issue aberta fica,
    /// local e remota, mesmo com o PR mesclado no head.
    #[test]
    fn epico_com_sub_issue_aberta_fica() {
        let e = setup();
        let (wi, tip) = e.branch(40, "wip/40-epico");
        e.pr(400, wi, "wip/40-epico", PrState::Merged, Some(&tip));
        e.publish("wip/40-epico", &tip);
        e.env
            .sessions
            .ledger()
            .apply_github(&GithubSnapshot {
                project_id: e.env.project_id,
                issues: vec![IssueRow {
                    gh_number: 41,
                    title: "filha".into(),
                    state: "open".into(),
                    labels: vec![],
                    parent_number: Some(40),
                    has_sub_issues: false,
                    url: String::new(),
                    updated_at: 1,
                    sprints: Vec::new(),
                }],
                prs: vec![],
                etags: vec![],
                synced_at: 1,
            })
            .unwrap();
        let c = e.cleaner();
        let plan = e.plan(&c);
        assert_eq!(
            reason(&step(&plan, wi, StepKind::BranchLocal)),
            "wip/40-epico não removida: branch de épico com sub-issue aberta (#41)"
        );
        e.run(&c, false);
        assert_eq!(e.tip("wip/40-epico"), Some(tip));
        assert!(e.deletes().is_empty());
    }

    /// Projeto Jira + GitLab no `Env`, com o glab falso no lugar do GitHub.
    struct Gitlab {
        e: Env,
        glab: ct_gitlab::fake::FakeGlab,
    }

    const GL_HOST: &str = "git.exemplo.test";

    fn setup_gitlab() -> Gitlab {
        let e = setup();
        let glab = ct_gitlab::fake::FakeGlab::new(e.env.tmp.path(), GL_HOST, "tok-gl");
        e.env.assign_provider(
            ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
                url: "https://jira.exemplo.test".into(),
                project_key: "PROJ".into(),
                ..Default::default()
            }),
            ct_core::ForgeConfig::Gitlab(ct_core::GitlabConfig {
                host: GL_HOST.into(),
                path: format!("{OWNER}/{NAME}"),
                clone_url: format!("https://{GL_HOST}/{OWNER}/{NAME}.git"),
                credential_cmd: "printf 'tok-gl\\n'".into(),
            }),
            false,
            1,
        );
        Gitlab { e, glab }
    }

    impl Gitlab {
        fn cleaner(&self) -> Cleaner {
            let remote = crate::gitlab::GitlabRemote::new(self.glab.bin().to_str().unwrap());
            Cleaner::standard(cfg())
                .with_github(Some(Arc::clone(&self.e.github)))
                .with_gitlab(Some(Arc::new(remote)))
        }
    }

    /// Critério de aceite da #350: MR mesclado no GitLab leva à limpeza da
    /// branch local e da remota, apagada no GitLab pelo glab (com o token do
    /// comando de credencial), sem tocar no GitHub.
    #[test]
    fn mr_mesclado_no_gitlab_sai_local_e_remota() {
        let g = setup_gitlab();
        let e = &g.e;
        let (wi, tip) = e.branch(9, "PROJ-9");
        e.pr(90, wi, "PROJ-9", PrState::Merged, Some(&tip));
        g.glab.set_branch("PROJ-9", &tip);
        let c = g.cleaner();
        let plan = e.plan(&c);
        let local = step(&plan, wi, StepKind::BranchLocal);
        assert_eq!(
            local.targets,
            vec![format!(
                "branch local PROJ-9 ({}, igual ao head do MR !90 mesclado)",
                &tip[..7]
            )]
        );
        let remota = step(&plan, wi, StepKind::BranchRemota);
        assert_eq!(remota.state, StepState::Pendente, "{remota:?}");
        assert_eq!(
            remota.targets,
            vec![format!(
                "branch remota origin/PROJ-9 ({}, igual ao head do MR !90 mesclado)",
                &tip[..7]
            )]
        );

        git_in(&e.origin, &["branch", "-D", "PROJ-9"]);
        let r = e.run(&c, false);
        let s = &r.items[0].steps;
        assert_eq!(
            (s[0].result, s[1].result),
            (StepResult::Feita, StepResult::Feita),
            "{r:?}"
        );
        assert_eq!(e.tip("PROJ-9"), None);
        assert_eq!(e.remote("PROJ-9"), None);
        assert_eq!(
            g.glab.deletes(),
            vec!["projects/deep-ion-ai%2Fdemo/repository/branches/PROJ-9"]
        );
        assert!(e.deletes().is_empty(), "nada no GitHub");
        let calls = g.glab.calls();
        assert!(
            calls
                .iter()
                .all(|c| c.contains(&format!("host={GL_HOST}")) && !c.contains("tok-gl")),
            "{calls:?}"
        );
    }

    /// No GitLab, push depois do MR segura a remota, na execução também.
    #[test]
    fn gitlab_confere_a_ponta_antes_de_apagar() {
        let g = setup_gitlab();
        let e = &g.e;
        let (wi, head) = e.branch(9, "PROJ-9");
        e.pr(90, wi, "PROJ-9", PrState::Closed, Some(&head));
        g.glab
            .set_branch("PROJ-9", "f00f00f00f00f00f00f00f00f00f00f00f00f00f");
        let c = g.cleaner();
        let r = e.run(&c, false);
        let s = &r.items[0].steps[1];
        assert_eq!(
            (s.kind, s.result),
            (StepKind::BranchRemota, StepResult::Falhou)
        );
        let why = s.reason.as_deref().unwrap();
        assert!(
            why.contains("no GitLab está em f00f00f") && why.contains("head do MR"),
            "{why}"
        );
        assert!(g.glab.deletes().is_empty());

        // Sem o glab configurado, a remota fica com o motivo.
        let sem = Cleaner::standard(cfg()).with_github(Some(Arc::clone(&e.github)));
        let (w2, _) = e.branch(10, "PROJ-10");
        e.pr(100, w2, "PROJ-10", PrState::Merged, None);
        let plan = e.plan(&sem);
        assert_eq!(
            reason(&step(&plan, w2, StepKind::BranchLocal)),
            "PROJ-10 não removida: head do MR !100 desconhecido (sync com o GitLab desativado)"
        );
    }

    /// A branch em uso no clone principal fica, com o motivo.
    #[test]
    fn branch_em_uso_no_clone_principal_fica() {
        let e = setup();
        let (wi, tip) = e.branch(9, "feat/9-x");
        e.pr(90, wi, "feat/9-x", PrState::Merged, Some(&tip));
        git_in(e.repo(), &["checkout", "-q", "feat/9-x"]);
        let plan = e.plan(&e.cleaner());
        let local = step(&plan, wi, StepKind::BranchLocal);
        assert!(reason(&local).contains("em uso no worktree"), "{local:?}");
    }
}
