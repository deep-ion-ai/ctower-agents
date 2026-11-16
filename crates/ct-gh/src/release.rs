//! Etapas determinísticas da release (#204), sobre a API do GitHub.
//!
//! Cada etapa confere pré-condições fixas e devolve um [`StepOutcome`]; os
//! merges são sempre `merge_method: "merge"` com o `sha` do head conferido
//! (o GitHub recusa com `409` se o head mudou), nunca squash, rebase, push
//! nem bypass de regras. As etapas são idempotentes: rodar de novo depois de
//! sucesso devolve `noop`. A idempotência vem do estado no GitHub.
//!
//! O fluxo é o do release-please (`docs/release.md`):
//! promoção `source → target` → PR de release (`release_pr_branch → target`)
//! → publicação da release → back-merge `target → source`.

use crate::refs::{encode_branch, pull_path};
use crate::sync::{encode, open_pulls_path};
use crate::{GhClient, GhError};
use ct_core::release::{
    green_conclusion, version_from_title, ReleaseConfig, ReleaseInfo, ReleasePr, ReleaseRun,
    ReleaseStatus, ReleaseStep, StepOutcome, StepResult,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

/// Ritmo das esperas. Em produção, uma consulta a cada 15 s e minutos de
/// 60 s; os testes encurtam os dois.
#[derive(Debug, Clone, Copy)]
pub struct Pace {
    pub interval: Duration,
    /// Duração de um "minuto" dos prazos de [`ReleaseConfig::timeouts`].
    pub minute: Duration,
}

impl Default for Pace {
    fn default() -> Self {
        Pace {
            interval: Duration::from_secs(15),
            minute: Duration::from_secs(60),
        }
    }
}

/// Título padrão do PR de promoção.
pub fn promotion_title(cfg: &ReleaseConfig) -> String {
    format!("release: {} -> {}", cfg.source, cfg.target)
}

/// A release de um repositório, com a configuração do projeto.
pub struct Release<'a> {
    api: &'a GhClient,
    owner: String,
    repo: String,
    cfg: ReleaseConfig,
    pace: Pace,
}

/// Estado dos checks obrigatórios num commit.
#[derive(Debug, Default)]
struct Checks {
    pending: Vec<String>,
    failing: Vec<String>,
}

/// Um passo de espera: pronto, ainda não, ou desfecho final.
enum Poll<T> {
    Ready(T),
    NotYet(String),
    Done(StepOutcome),
}

/// Branch: SHA do commit e da árvore, e o JSON (para a proteção).
struct Branch {
    sha: String,
    tree: String,
    json: Value,
}

impl<'a> Release<'a> {
    pub fn new(api: &'a GhClient, owner: &str, repo: &str, cfg: &ReleaseConfig) -> Self {
        Release {
            api,
            owner: owner.to_string(),
            repo: repo.to_string(),
            cfg: cfg.normalized(),
            pace: Pace::default(),
        }
    }

    pub fn with_pace(mut self, pace: Pace) -> Self {
        self.pace = pace;
        self
    }

    pub fn config(&self) -> &ReleaseConfig {
        &self.cfg
    }

    // ---- leitura ---------------------------------------------------------

    fn repo_path(&self, rest: &str) -> String {
        format!("/repos/{}/{}{rest}", self.owner, self.repo)
    }

    fn branch(&self, name: &str) -> Result<Branch, GhError> {
        let v = self
            .api
            .get_json(&self.repo_path(&format!("/branches/{}", encode_branch(name))))?
            .ok_or_else(|| GhError::Http {
                status: 404,
                message: format!("branch {name} não existe em {}/{}", self.owner, self.repo),
            })?;
        let sha = str_at(&v, "/commit/sha")
            .ok_or_else(|| GhError::Parse(format!("branch {name} sem commit.sha")))?;
        let tree = str_at(&v, "/commit/commit/tree/sha")
            .ok_or_else(|| GhError::Parse(format!("branch {name} sem commit.tree.sha")))?;
        Ok(Branch { sha, tree, json: v })
    }

    /// `(ahead_by, behind_by)` de `head` em relação a `base`.
    fn compare(&self, base: &str, head: &str) -> Result<(i64, i64), GhError> {
        let v = self
            .api
            .get_json(&self.repo_path(&format!(
                "/compare/{}...{}",
                encode_branch(base),
                encode_branch(head)
            )))?
            .ok_or_else(|| GhError::Parse(format!("compare {base}...{head} não existe")))?;
        let n = |k: &str| v.get(k).and_then(Value::as_i64).unwrap_or(0);
        Ok((n("ahead_by"), n("behind_by")))
    }

    fn open_pr(&self, head: &str, base: &str) -> Result<Option<ReleasePr>, GhError> {
        let list = match self
            .api
            .get_all(&open_pulls_path(&self.owner, &self.repo, head, base), None)?
        {
            crate::Fetched::Modified { body, .. } => body,
            crate::Fetched::NotModified => vec![],
        };
        Ok(list.iter().filter_map(to_pr).min_by_key(|p| p.number))
    }

    /// O PR fechado mais recente `head → base`, com `merged_at`.
    fn last_closed_pr(&self, head: &str, base: &str) -> Result<Option<(ReleasePr, bool)>, GhError> {
        let path = self.repo_path(&format!(
            "/pulls?state=closed&head={}&base={}&sort=updated&direction=desc&per_page=20",
            encode(&format!("{}:{head}", self.owner)),
            encode(base)
        ));
        let list = self.api.get_json(&path)?.unwrap_or(Value::Null);
        Ok(list.as_array().and_then(|a| {
            a.iter().find_map(|v| {
                let merged = v.get("merged_at").is_some_and(|m| !m.is_null());
                to_pr(v).map(|p| (p, merged))
            })
        }))
    }

    fn pull(&self, number: i64) -> Result<Value, GhError> {
        self.api
            .get_json(&pull_path(&self.owner, &self.repo, number))?
            .ok_or_else(|| GhError::Http {
                status: 404,
                message: format!("PR #{number} não existe"),
            })
    }

    fn runs(&self, sha: &str) -> Result<Vec<ReleaseRun>, GhError> {
        let v = self
            .api
            .get_json(&self.repo_path(&format!("/actions/runs?head_sha={sha}&per_page=100")))?
            .unwrap_or(Value::Null);
        Ok(v.get("workflow_runs")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(to_run).collect())
            .unwrap_or_default())
    }

    fn releases(&self) -> Result<Vec<ReleaseInfo>, GhError> {
        let v = self
            .api
            .get_json(&self.repo_path("/releases?per_page=20"))?
            .unwrap_or(Value::Null);
        Ok(v.as_array()
            .map(|a| a.iter().filter_map(to_release).collect())
            .unwrap_or_default())
    }

    /// Contextos obrigatórios em `branch`: a proteção clássica (no JSON da
    /// branch) e os rulesets (`/rules/branches/<b>`).
    fn required_contexts(&self, branch: &Branch, name: &str) -> Result<Vec<String>, GhError> {
        let mut out = BTreeSet::new();
        let rsc = branch.json.pointer("/protection/required_status_checks");
        for c in rsc
            .and_then(|r| r.get("contexts"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(s) = c.as_str() {
                out.insert(s.to_string());
            }
        }
        for c in rsc
            .and_then(|r| r.get("checks"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(s) = c.get("context").and_then(Value::as_str) {
                out.insert(s.to_string());
            }
        }
        let rules = self
            .api
            .get_json(&self.repo_path(&format!("/rules/branches/{}", encode_branch(name))))?
            .unwrap_or(Value::Null);
        for rule in rules.as_array().into_iter().flatten() {
            if rule.get("type").and_then(Value::as_str) != Some("required_status_checks") {
                continue;
            }
            for c in rule
                .pointer("/parameters/required_status_checks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if let Some(s) = c.get("context").and_then(Value::as_str) {
                    out.insert(s.to_string());
                }
            }
        }
        Ok(out.into_iter().collect())
    }

    /// Estado dos contextos `required` no commit `sha` (check runs e
    /// statuses). Ausente conta como pendente.
    fn checks(&self, sha: &str, required: &[String]) -> Result<Checks, GhError> {
        let mut c = Checks::default();
        if required.is_empty() {
            return Ok(c);
        }
        let runs = self
            .api
            .get_json(&self.repo_path(&format!("/commits/{sha}/check-runs?per_page=100")))?
            .unwrap_or(Value::Null);
        let statuses = self
            .api
            .get_json(&self.repo_path(&format!("/commits/{sha}/status")))?
            .unwrap_or(Value::Null);
        for ctx in required {
            // O check run mais recente com o nome (maior id).
            let run = runs
                .get("check_runs")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|r| r.get("name").and_then(Value::as_str) == Some(ctx))
                .max_by_key(|r| r.get("id").and_then(Value::as_i64).unwrap_or(0));
            let status = statuses
                .get("statuses")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .find(|s| s.get("context").and_then(Value::as_str) == Some(ctx));
            if let Some(r) = run {
                let done = r.get("status").and_then(Value::as_str) == Some("completed");
                let concl = r.get("conclusion").and_then(Value::as_str);
                if !done {
                    c.pending.push(ctx.clone());
                } else if !green_conclusion(concl) {
                    c.failing.push(format!("{ctx} ({})", concl.unwrap_or("?")));
                }
            } else if let Some(s) = status {
                match s.get("state").and_then(Value::as_str) {
                    Some("success") => {}
                    Some("pending") | None => c.pending.push(ctx.clone()),
                    Some(other) => c.failing.push(format!("{ctx} ({other})")),
                }
            } else {
                c.pending.push(ctx.clone());
            }
        }
        Ok(c)
    }

    // ---- status ----------------------------------------------------------

    /// Estado da release: diferença de conteúdo, PRs abertos do fluxo, runs
    /// pendentes, a release mais recente e a próxima etapa.
    pub fn status(&self) -> Result<ReleaseStatus, GhError> {
        let c = &self.cfg;
        let source = self.branch(&c.source)?;
        let target = self.branch(&c.target)?;
        let (target_ahead_by, source_ahead_by) = self.compare(&c.source, &c.target)?;
        let promotion_pr = self.open_pr(&c.source, &c.target)?;
        let release_pr = self.open_pr(&c.release_pr_head(), &c.target)?;
        let back_merge_pr = if c.back_merge {
            self.open_pr(&c.target, &c.source)?
        } else {
            None
        };
        let mut pending_runs = Vec::new();
        for pr in [&promotion_pr, &release_pr, &back_merge_pr]
            .into_iter()
            .flatten()
        {
            pending_runs.extend(
                self.runs(&pr.head_sha)?
                    .into_iter()
                    .filter(ReleaseRun::pending_approval),
            );
        }
        let latest_release = self.releases()?.into_iter().next();
        let mut s = ReleaseStatus {
            owner: self.owner.clone(),
            repo: self.repo.clone(),
            source: c.source.clone(),
            target: c.target.clone(),
            source_sha: source.sha,
            target_sha: target.sha,
            content_diff: source.tree != target.tree,
            target_ahead_by,
            source_ahead_by,
            promotion_pr,
            release_pr,
            back_merge_pr,
            pending_runs,
            latest_release,
            next: ReleaseStep::Nothing,
        };
        s.next = s.next_step();
        Ok(s)
    }

    // ---- etapas ----------------------------------------------------------

    /// `promote`: abre (ou reaproveita) o PR `source → target` com `body`.
    /// Recusa sem diferença de conteúdo ou com back-merge pendente.
    pub fn promote(&self, title: Option<&str>, body: &str) -> StepOutcome {
        self.step(ReleaseStep::Promote, || {
            let c = &self.cfg;
            let st = self.status()?;
            if let Some(pr) = &st.back_merge_pr {
                return Ok(refused(
                    ReleaseStep::Promote,
                    format!(
                        "há back-merge pendente (PR #{} {} → {}): mescle-o primeiro com \
                         `ctower release merge-back`",
                        pr.number, c.target, c.source
                    ),
                )
                .with_pr(pr));
            }
            if st.target_ahead_by > 0 && c.back_merge {
                return Ok(refused(
                    ReleaseStep::Promote,
                    format!(
                        "{} tem {} commit(s) que {} não tem: falta o back-merge da release \
                         anterior (`ctower release merge-back`)",
                        c.target, st.target_ahead_by, c.source
                    ),
                ));
            }
            if let Some(pr) = &st.promotion_pr {
                if !body.trim().is_empty() {
                    self.api.patch_json(
                        &pull_path(&self.owner, &self.repo, pr.number),
                        &json!({ "body": body }),
                    )?;
                }
                return Ok(StepOutcome::new(
                    ReleaseStep::Promote,
                    StepResult::Noop,
                    format!("PR de promoção já aberto: #{}", pr.number),
                )
                .with_pr(pr));
            }
            if !st.content_diff {
                return Ok(refused(
                    ReleaseStep::Promote,
                    format!(
                        "nada a lançar: {} e {} têm o mesmo conteúdo",
                        c.source, c.target
                    ),
                ));
            }
            let title = title
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(String::from)
                .unwrap_or_else(|| promotion_title(c));
            let v = self.api.post_json(
                &self.repo_path("/pulls"),
                &json!({ "title": title, "head": c.source, "base": c.target, "body": body }),
            )?;
            let pr = to_pr(&v).ok_or_else(|| GhError::Parse("PR criado sem número".into()))?;
            Ok(StepOutcome::new(
                ReleaseStep::Promote,
                StepResult::Ok,
                format!("PR de promoção aberto: #{}", pr.number),
            )
            .with_pr(&pr))
        })
    }

    /// `merge-promotion`: mescla o PR `source → target` (merge commit, preso
    /// ao SHA, checks obrigatórios verdes).
    pub fn merge_promotion(&self, expected_sha: Option<&str>) -> StepOutcome {
        let c = &self.cfg;
        self.merge_flow_pr(
            ReleaseStep::MergePromotion,
            &c.source,
            &c.target,
            expected_sha,
            false,
            "não há PR de promoção aberto: rode `ctower release promote`",
        )
    }

    /// `merge-release`: mescla o PR de release (merge commit, preso ao SHA,
    /// checks obrigatórios e, com `approve_pending_runs`, todos os runs
    /// verdes).
    pub fn merge_release(&self, expected_sha: Option<&str>) -> StepOutcome {
        let c = &self.cfg;
        self.merge_flow_pr(
            ReleaseStep::MergeRelease,
            &c.release_pr_head(),
            &c.target,
            expected_sha,
            c.approve_pending_runs,
            "não há PR de release aberto: rode `ctower release wait-release-pr`",
        )
    }

    /// `wait-release-pr`: espera o workflow do `target` terminar no head
    /// atual (o release-please e o commit dos lockfiles) e o PR de release
    /// aberto.
    pub fn wait_release_pr(&self) -> StepOutcome {
        let step = ReleaseStep::WaitReleasePr;
        self.step(step, || {
            let c = &self.cfg;
            let head = c.release_pr_head();
            let r = self.poll(c.timeouts.release_pr, || {
                let target = self.branch(&c.target)?;
                let runs: Vec<ReleaseRun> = self.runs(&target.sha)?;
                if runs.is_empty() {
                    return Ok(Poll::NotYet(format!(
                        "nenhum run do workflow no head de {} ({})",
                        c.target,
                        short(&target.sha)
                    )));
                }
                if let Some(r) = runs.iter().find(|r| !r.completed()) {
                    return Ok(Poll::NotYet(format!(
                        "run {} ainda em {}",
                        r.name, r.status
                    )));
                }
                let failed: Vec<&ReleaseRun> = runs.iter().filter(|r| !r.green()).collect();
                if !failed.is_empty() {
                    return Ok(Poll::Done(
                        StepOutcome::new(
                            step,
                            StepResult::Failed,
                            format!(
                                "o workflow de {} falhou: {}; reexecute os jobs que falharam \
                                 (`gh run rerun <id> --failed`) e rode a etapa de novo",
                                c.target,
                                run_names(&failed)
                            ),
                        )
                        .with_sha(&target.sha)
                        .with_details(json!({ "failed_runs": failed })),
                    ));
                }
                match self.open_pr(&head, &c.target)? {
                    Some(pr) => Ok(Poll::Ready(pr)),
                    None => Ok(Poll::Done(
                        refused(
                            step,
                            format!(
                                "o workflow de {} terminou sem PR de release ({head} → {}): \
                                 os commits não geram versão?",
                                c.target, c.target
                            ),
                        )
                        .with_sha(&target.sha),
                    )),
                }
            })?;
            let pr = match r {
                Ok(pr) => pr,
                Err(o) => return Ok(o),
            };
            let version = version_from_title(&pr.title);
            Ok(StepOutcome::new(
                step,
                StepResult::Ok,
                format!(
                    "PR de release #{} pronto{}",
                    pr.number,
                    version
                        .as_deref()
                        .map(|v| format!(" (versão {v})"))
                        .unwrap_or_default()
                ),
            )
            .with_pr(&pr)
            .with_details(json!({ "version": version })))
        })
    }

    /// `approve-runs <pr>`: aprova os runs em `action_required` do head
    /// atual do PR, espera terminarem e reexecuta uma vez os que falharam.
    pub fn approve_runs(&self, number: i64) -> StepOutcome {
        let step = ReleaseStep::ApproveRuns;
        self.step(step, || {
            if !self.cfg.approve_pending_runs {
                return Ok(refused(
                    step,
                    "a aprovação de runs está desligada na configuração de release do projeto",
                ));
            }
            let pr = self.pull(number)?;
            if pr.get("state").and_then(Value::as_str) != Some("open") {
                return Ok(refused(step, format!("o PR #{number} não está aberto")));
            }
            let base = str_at(&pr, "/base/ref").unwrap_or_default();
            if !self.flow_base(&base) {
                return Ok(refused(
                    step,
                    format!(
                        "o PR #{number} não é do fluxo de release (base {base}, esperado {} ou \
                         {})",
                        self.cfg.target, self.cfg.source
                    ),
                ));
            }
            self.approve_and_wait(number)
        })
    }

    /// Aprova, espera e reexecuta os runs do PR até todos ficarem verdes.
    fn approve_and_wait(&self, number: i64) -> Result<StepOutcome, GhError> {
        let step = ReleaseStep::ApproveRuns;
        let mut approved: Vec<i64> = Vec::new();
        let mut reran: Vec<i64> = Vec::new();
        let mut empty_once = false;
        let r = self.poll(self.cfg.timeouts.runs, || {
            let pr = self.pull(number)?;
            let sha = str_at(&pr, "/head/sha").unwrap_or_default();
            let runs = self.runs(&sha)?;
            if runs.is_empty() {
                if empty_once {
                    return Ok(Poll::Ready((sha, runs)));
                }
                empty_once = true;
                return Ok(Poll::NotYet("nenhum run no head ainda".into()));
            }
            let mut acted = false;
            for r in runs.iter().filter(|r| r.pending_approval()) {
                self.api.post_json(
                    &self.repo_path(&format!("/actions/runs/{}/approve", r.id)),
                    &json!({}),
                )?;
                approved.push(r.id);
                acted = true;
            }
            if acted {
                return Ok(Poll::NotYet("runs aprovados; esperando terminarem".into()));
            }
            if let Some(r) = runs.iter().find(|r| !r.completed()) {
                return Ok(Poll::NotYet(format!("run {} em {}", r.name, r.status)));
            }
            let failed: Vec<&ReleaseRun> = runs.iter().filter(|r| !r.green()).collect();
            if failed.is_empty() {
                return Ok(Poll::Ready((sha, runs)));
            }
            // Reexecuta uma vez cada run que falhou na primeira tentativa
            // (corrida conhecida: o check de versões antes do commit dos
            // lockfiles).
            let retry: Vec<&&ReleaseRun> = failed
                .iter()
                .filter(|r| r.attempt <= 1 && !reran.contains(&r.id))
                .collect();
            if retry.is_empty() {
                return Ok(Poll::Done(
                    StepOutcome::new(
                        step,
                        StepResult::Failed,
                        format!(
                            "runs vermelhos no PR #{number} depois de reexecutar: {}",
                            run_names(&failed)
                        ),
                    )
                    .with_sha(&sha)
                    .with_details(json!({
                        "approved": approved, "reran": reran, "failed_runs": failed
                    })),
                ));
            }
            for r in retry {
                self.api.post_json(
                    &self.repo_path(&format!("/actions/runs/{}/rerun-failed-jobs", r.id)),
                    &json!({}),
                )?;
                reran.push(r.id);
            }
            Ok(Poll::NotYet(
                "runs reexecutados; esperando terminarem".into(),
            ))
        })?;
        let (sha, runs) = match r {
            Ok(v) => v,
            Err(o) => return Ok(o.with_details(json!({ "approved": approved, "reran": reran }))),
        };
        let result = if approved.is_empty() && reran.is_empty() {
            StepResult::Noop
        } else {
            StepResult::Ok
        };
        Ok(StepOutcome::new(
            step,
            result,
            format!(
                "PR #{number}: {} run(s) verde(s), {} aprovado(s), {} reexecutado(s)",
                runs.len(),
                approved.len(),
                reran.len()
            ),
        )
        .with_sha(sha)
        .with_details(json!({ "pr": number, "approved": approved, "reran": reran })))
    }

    /// `wait-publish`: espera a tag da versão do último PR de release
    /// mesclado, os runs do commit de merge e a release fora do rascunho.
    pub fn wait_publish(&self) -> StepOutcome {
        let step = ReleaseStep::WaitPublish;
        self.step(step, || {
            let c = &self.cfg;
            let Some((pr, merged)) = self.last_closed_pr(&c.release_pr_head(), &c.target)? else {
                return Ok(refused(step, "nenhum PR de release mesclado"));
            };
            if !merged {
                return Ok(refused(
                    step,
                    format!(
                        "o último PR de release (#{}) foi fechado sem merge",
                        pr.number
                    ),
                ));
            }
            let Some(version) = version_from_title(&pr.title) else {
                return Ok(refused(
                    step,
                    format!("sem versão no título do PR #{} ({})", pr.number, pr.title),
                ));
            };
            let merge_sha = str_at(&self.pull(pr.number)?, "/merge_commit_sha");
            let mut waited = false;
            let r = self.poll(c.timeouts.publish, || {
                let rel = self
                    .releases()?
                    .into_iter()
                    .find(|r| tag_matches(&r.tag, &version));
                if let Some(rel) = rel.as_ref().filter(|r| !r.draft) {
                    return Ok(Poll::Ready(rel.clone()));
                }
                waited = true;
                if let Some(sha) = &merge_sha {
                    let runs = self.runs(sha)?;
                    let failed: Vec<&ReleaseRun> = runs
                        .iter()
                        .filter(|r| r.completed() && !r.green())
                        .collect();
                    if !failed.is_empty() && runs.iter().all(ReleaseRun::completed) {
                        return Ok(Poll::Done(
                            StepOutcome::new(
                                step,
                                StepResult::Failed,
                                format!(
                                    "a publicação de v{version} falhou: {}; reexecute os jobs \
                                     que falharam (`gh run rerun <id> --failed`) e rode a etapa \
                                     de novo",
                                    run_names(&failed)
                                ),
                            )
                            .with_sha(sha)
                            .with_details(json!({ "failed_runs": failed })),
                        ));
                    }
                }
                Ok(Poll::NotYet(match rel {
                    Some(_) => format!("release v{version} ainda em rascunho"),
                    None => format!("tag v{version} ainda sem release"),
                }))
            })?;
            match r {
                Ok(rel) => Ok(StepOutcome::new(
                    step,
                    if waited {
                        StepResult::Ok
                    } else {
                        StepResult::Noop
                    },
                    format!("release {} publicada: {}", rel.tag, rel.url),
                )
                .with_details(json!({ "version": version, "release": rel, "pr": pr.number }))),
                Err(o) => Ok(o),
            }
        })
    }

    /// `merge-back`: espera o PR `target → source`, aprova os runs dele e
    /// mescla com merge commit, preso ao SHA.
    pub fn merge_back(&self, expected_sha: Option<&str>) -> StepOutcome {
        let step = ReleaseStep::MergeBack;
        self.step(step, || {
            let c = &self.cfg;
            if !c.back_merge {
                return Ok(StepOutcome::new(
                    step,
                    StepResult::Noop,
                    "back-merge desligado na configuração de release do projeto",
                ));
            }
            let r = self.poll(c.timeouts.back_merge, || {
                if let Some(pr) = self.open_pr(&c.target, &c.source)? {
                    return Ok(Poll::Ready(pr));
                }
                let (target_ahead, _) = self.compare(&c.source, &c.target)?;
                if target_ahead == 0 {
                    return Ok(Poll::Done(StepOutcome::new(
                        step,
                        StepResult::Noop,
                        format!("{} já contém {}: nada a voltar", c.source, c.target),
                    )));
                }
                Ok(Poll::NotYet(format!(
                    "esperando o PR de back-merge {} → {}",
                    c.target, c.source
                )))
            })?;
            let pr = match r {
                Ok(pr) => pr,
                Err(o) => return Ok(o),
            };
            if c.approve_pending_runs {
                let o = self.approve_and_wait(pr.number)?;
                if o.result == StepResult::Failed || o.result == StepResult::Refused {
                    return Ok(StepOutcome { step, ..o });
                }
            }
            Ok(self.merge_pr(
                step,
                pr.number,
                &c.source,
                expected_sha,
                c.approve_pending_runs,
            ))
        })
    }

    // ---- merge -----------------------------------------------------------

    fn flow_base(&self, base: &str) -> bool {
        base == self.cfg.target || base == self.cfg.source
    }

    /// Mescla o PR aberto `head → base` do fluxo ou, sem PR aberto, confere
    /// se o último já foi mesclado (`noop`).
    fn merge_flow_pr(
        &self,
        step: ReleaseStep,
        head: &str,
        base: &str,
        expected_sha: Option<&str>,
        require_runs: bool,
        none_msg: &str,
    ) -> StepOutcome {
        let open = match self.open_pr(head, base) {
            Ok(o) => o,
            Err(e) => return failed(step, e),
        };
        match open {
            Some(pr) => self.merge_pr(step, pr.number, base, expected_sha, require_runs),
            None => match self.last_closed_pr(head, base) {
                Ok(Some((pr, true))) => StepOutcome::new(
                    step,
                    StepResult::Noop,
                    format!("PR #{} já mesclado", pr.number),
                )
                .with_pr(&pr),
                Ok(_) => refused(step, none_msg),
                Err(e) => failed(step, e),
            },
        }
    }

    /// Mescla o PR `number` com merge commit, preso ao head conferido:
    /// recusa se o head mudou, se há conflito ou se um check obrigatório de
    /// `base` (e, com `require_runs`, um run do head) não está verde.
    fn merge_pr(
        &self,
        step: ReleaseStep,
        number: i64,
        base: &str,
        expected_sha: Option<&str>,
        require_runs: bool,
    ) -> StepOutcome {
        self.step(step, || {
            let mut pr = self.pull(number)?;
            if pr.get("merged").and_then(Value::as_bool) == Some(true) {
                return Ok(StepOutcome::new(
                    step,
                    StepResult::Noop,
                    format!("PR #{number} já mesclado"),
                ));
            }
            if pr.get("state").and_then(Value::as_str) != Some("open") {
                return Ok(refused(
                    step,
                    format!("o PR #{number} foi fechado sem merge"),
                ));
            }
            let sha = str_at(&pr, "/head/sha")
                .ok_or_else(|| GhError::Parse(format!("PR #{number} sem head.sha")))?;
            if let Some(want) = expected_sha {
                if sha != want && !(want.len() >= 7 && sha.starts_with(want)) {
                    return Ok(refused(
                        step,
                        format!(
                            "o head do PR #{number} mudou desde a conferência ({} → {}): \
                             confira de novo",
                            short(want),
                            short(&sha)
                        ),
                    )
                    .with_sha(&sha));
                }
            }
            // `mergeable` vem `null` enquanto o GitHub calcula.
            let mut tries = 0;
            while pr.get("mergeable").is_some_and(Value::is_null) && tries < 5 {
                std::thread::sleep(self.pace.interval);
                pr = self.pull(number)?;
                tries += 1;
            }
            let state = str_at(&pr, "/mergeable_state").unwrap_or_default();
            if pr.get("mergeable").and_then(Value::as_bool) == Some(false) || state == "dirty" {
                return Ok(refused(
                    step,
                    format!("o PR #{number} tem conflito com {base}: resolva fora da sessão"),
                )
                .with_sha(&sha));
            }
            let base_branch = self.branch(base)?;
            let required = self.required_contexts(&base_branch, base)?;
            let waited = self.poll(self.cfg.timeouts.runs, || {
                let checks = self.checks(&sha, &required)?;
                if !checks.failing.is_empty() {
                    return Ok(Poll::Done(
                        refused(
                            step,
                            format!(
                                "check obrigatório vermelho no PR #{number}: {}",
                                checks.failing.join(", ")
                            ),
                        )
                        .with_sha(&sha),
                    ));
                }
                if !checks.pending.is_empty() {
                    return Ok(Poll::NotYet(format!(
                        "checks obrigatórios pendentes: {}",
                        checks.pending.join(", ")
                    )));
                }
                if require_runs {
                    let runs = self.runs(&sha)?;
                    if runs.iter().any(ReleaseRun::pending_approval) {
                        return Ok(Poll::Done(
                            refused(
                                step,
                                format!(
                                    "há runs esperando aprovação no PR #{number}: rode \
                                     `ctower release approve-runs {number}`"
                                ),
                            )
                            .with_sha(&sha),
                        ));
                    }
                    if let Some(r) = runs.iter().find(|r| !r.completed()) {
                        return Ok(Poll::NotYet(format!("run {} em {}", r.name, r.status)));
                    }
                    let red: Vec<&ReleaseRun> = runs.iter().filter(|r| !r.green()).collect();
                    if !red.is_empty() {
                        return Ok(Poll::Done(
                            refused(
                                step,
                                format!("runs vermelhos no PR #{number}: {}", run_names(&red)),
                            )
                            .with_sha(&sha),
                        ));
                    }
                }
                Ok(Poll::Ready(()))
            })?;
            if let Err(o) = waited {
                return Ok(o);
            }
            let r = self.api.put_json(
                &format!("{}/merge", pull_path(&self.owner, &self.repo, number)),
                &json!({ "merge_method": "merge", "sha": sha }),
            );
            match r {
                Ok(v) => Ok(StepOutcome::new(
                    step,
                    StepResult::Ok,
                    format!("PR #{number} mesclado com merge commit"),
                )
                .with_sha(&sha)
                .with_details(json!({
                    "pr": number,
                    "merge_commit": v.get("sha").and_then(Value::as_str),
                }))),
                Err(GhError::Http { status: 409, .. }) => Ok(refused(
                    step,
                    format!("o head do PR #{number} mudou desde a conferência: confira de novo"),
                )
                .with_sha(&sha)),
                Err(GhError::Http {
                    status: 405,
                    message,
                }) => Ok(refused(
                    step,
                    format!("o GitHub recusou o merge do PR #{number}: {message}"),
                )
                .with_sha(&sha)),
                Err(e) => Err(e),
            }
        })
    }

    // ---- infraestrutura --------------------------------------------------

    /// Roda a etapa; erro do GitHub vira `failed`. Preenche a etapa do
    /// prazo esgotado e o `pr` dos detalhes quando a etapa não o fez.
    fn step(
        &self,
        step: ReleaseStep,
        f: impl FnOnce() -> Result<StepOutcome, GhError>,
    ) -> StepOutcome {
        match f() {
            Ok(mut o) => {
                // O prazo esgotado do `poll` sai sem etapa.
                if o.step == ReleaseStep::Status {
                    o.step = step;
                }
                if o.pr.is_none() {
                    o.pr = o.details.get("pr").and_then(Value::as_i64);
                }
                o
            }
            Err(e) => failed(step, e),
        }
    }

    /// Repete `f` a cada [`Pace::interval`] até ficar pronto, dar um
    /// desfecho ou passar `minutes`. `Err(StepOutcome)` = desfecho final
    /// (inclusive o prazo esgotado).
    fn poll<T>(
        &self,
        minutes: u32,
        mut f: impl FnMut() -> Result<Poll<T>, GhError>,
    ) -> Result<Result<T, StepOutcome>, GhError> {
        let deadline = Instant::now() + self.pace.minute * minutes;
        loop {
            match f()? {
                Poll::Ready(v) => return Ok(Ok(v)),
                Poll::Done(o) => return Ok(Err(o)),
                Poll::NotYet(why) => {
                    if Instant::now() >= deadline {
                        return Ok(Err(StepOutcome::new(
                            ReleaseStep::Status,
                            StepResult::Failed,
                            format!("prazo de {minutes} min esgotado: {why}"),
                        )
                        .with_details(json!({ "timeout": true }))));
                    }
                    std::thread::sleep(self.pace.interval);
                }
            }
        }
    }
}

fn refused(step: ReleaseStep, msg: impl Into<String>) -> StepOutcome {
    StepOutcome::new(step, StepResult::Refused, msg)
}

fn failed(step: ReleaseStep, e: GhError) -> StepOutcome {
    StepOutcome::new(step, StepResult::Failed, format!("erro do GitHub: {e}"))
}

fn str_at(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr).and_then(Value::as_str).map(String::from)
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

fn run_names(runs: &[&ReleaseRun]) -> String {
    runs.iter()
        .map(|r| {
            format!(
                "{} #{} ({})",
                r.name,
                r.id,
                r.conclusion.as_deref().unwrap_or(&r.status)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A tag da release é a da versão (`v1.2.3`, ou `componente-v1.2.3`).
fn tag_matches(tag: &str, version: &str) -> bool {
    tag == version || tag.ends_with(&format!("v{version}"))
}

fn to_pr(v: &Value) -> Option<ReleasePr> {
    Some(ReleasePr {
        number: v.get("number")?.as_i64()?,
        title: str_at(v, "/title").unwrap_or_default(),
        url: str_at(v, "/html_url").unwrap_or_default(),
        head: str_at(v, "/head/ref").unwrap_or_default(),
        base: str_at(v, "/base/ref").unwrap_or_default(),
        head_sha: str_at(v, "/head/sha").unwrap_or_default(),
    })
}

fn to_run(v: &Value) -> Option<ReleaseRun> {
    Some(ReleaseRun {
        id: v.get("id")?.as_i64()?,
        name: str_at(v, "/name").unwrap_or_default(),
        head_sha: str_at(v, "/head_sha").unwrap_or_default(),
        status: str_at(v, "/status").unwrap_or_default(),
        conclusion: str_at(v, "/conclusion"),
        attempt: v.get("run_attempt").and_then(Value::as_i64).unwrap_or(1),
        url: str_at(v, "/html_url").unwrap_or_default(),
    })
}

fn to_release(v: &Value) -> Option<ReleaseInfo> {
    Some(ReleaseInfo {
        tag: str_at(v, "/tag_name")?,
        name: str_at(v, "/name").unwrap_or_default(),
        url: str_at(v, "/html_url").unwrap_or_default(),
        draft: v.get("draft").and_then(Value::as_bool).unwrap_or(false),
    })
}

#[cfg(test)]
#[path = "release_tests.rs"]
mod tests;
