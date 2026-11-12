//! Modo autônomo no ambiente (épico #178, issue #181): a branch de
//! integração (`wip/autonoma`) e as sessões autônomas.
//!
//! - **Ligar o modo** ([`Sessions::autonomy_setup`]): o ambiente do modo
//!   cria a branch a partir da base, se faltar, e abre o PR rascunho
//!   `branch → base`. Roda na aplicação da configuração do hub. A branch
//!   recém-criada é igual à base, e o GitHub recusa o PR sem commits: o
//!   relato fica `ok` com o PR pendente (`pr_pending`), e cada alinhamento
//!   tenta de novo, até o primeiro merge na branch.
//! - **Alinhar** ([`Sessions::autonomy_align`]): traz a base para a branch
//!   (merge ou rebase, conforme o projeto). Conflito = o relato `conflict`
//!   sobe ao hub (aviso "wip/autonoma em conflito com a base") e o erro
//!   [`SessionsError::AutonomyConflict`] impede o início.
//! - **Sessão autônoma** ([`Sessions::start_autonomous`]): alinha e inicia a
//!   sessão da issue com a branch de integração como base da worktree; o
//!   agente recebe a instrução de abrir o PR contra ela.
//!
//! Só o ambiente do modo (`Project.autonomy_runner`) faz isso; nos outros, e
//! com o modo desligado, nada muda.

use crate::launch::StartRequest;
use crate::{Result, Sessions, SessionsError};
use ct_core::autonomy::{conflict_warning, draft_pr_pending_message};
use ct_core::{now_ms, AlignStrategy, AutonomyReport, AutonomyState};
use ct_git::integration::{align_branch, ensure_remote_branch, AlignOutcome, Strategy};
use ct_ledger::{NewEvent, Project, Session};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Mutex;

/// Evento no work item de uma sessão autônoma: `{ branch, base }`. A
/// retomada relê o mais recente para repetir a instrução ao agente.
pub const AUTONOMY_SESSION: &str = "autonomy_session";

/// Alerta (e evento no projeto) do alinhamento em conflito.
pub const AUTONOMY_CONFLICT: &str = "autonomy_conflict";

/// Um alinhamento por vez neste ambiente: dois inícios autônomos
/// simultâneos não disputam o mesmo push.
static ALIGN: Mutex<()> = Mutex::new(());

/// PR rascunho `head → base`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftPr {
    pub number: i64,
    pub url: String,
    /// Aberto agora (`false` = já existia).
    pub created: bool,
}

/// Quem abre o PR rascunho da branch de integração (o GitHub do ambiente).
pub trait PrOpener: Send + Sync {
    /// O PR aberto `head → base`, abrindo-o como rascunho se não existir.
    /// `Ok(None)` = ainda não dá para abrir: a forja recusa o PR porque
    /// `head` não tem commits além de `base` (a branch recém-criada).
    fn ensure_draft_pr(
        &self,
        project: &Project,
        head: &str,
        base: &str,
    ) -> std::result::Result<Option<DraftPr>, String>;
}

/// O `422` do GitHub para o PR sem commits entre as branches ("Validation
/// Failed: No commits between develop and wip/autonoma").
fn no_commits_between(e: &ct_gh::GhError) -> bool {
    matches!(e, ct_gh::GhError::Http { status: 422, message } if message.contains("No commits between"))
}

/// Título e corpo do PR rascunho da branch de integração.
fn draft_pr_body(head: &str, base: &str) -> Value {
    json!({
        "title": format!("Modo autônomo: {head} → {base}"),
        "head": head,
        "base": base,
        "draft": true,
        "body": format!(
            "Branch de integração do modo autônomo do Control Tower.\n\n\
             As sessões autônomas mesclam aqui as issues que implementam; revise e mescle em \
             `{base}` quando quiser. As issues mescladas em `{head}` ficam em revisão e concluem \
             quando este PR é mesclado."
        ),
    })
}

impl PrOpener for ct_gh::GhClient {
    fn ensure_draft_pr(
        &self,
        project: &Project,
        head: &str,
        base: &str,
    ) -> std::result::Result<Option<DraftPr>, String> {
        let (o, r) = (&project.gh_owner, &project.gh_repo);
        let path = ct_gh::sync::open_pulls_path(o, r, head, base);
        let open = match self.get_all(&path, None).map_err(|e| e.to_string())? {
            ct_gh::Fetched::Modified { body, .. } => body,
            ct_gh::Fetched::NotModified => Vec::new(),
        };
        let pr = |v: &Value, created| -> std::result::Result<DraftPr, String> {
            Ok(DraftPr {
                number: v
                    .get("number")
                    .and_then(Value::as_i64)
                    .ok_or("resposta do GitHub sem o número do PR")?,
                url: v
                    .get("html_url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                created,
            })
        };
        if let Some(v) = open.first() {
            return pr(v, false).map(Some);
        }
        match self.post_json(&format!("/repos/{o}/{r}/pulls"), &draft_pr_body(head, base)) {
            Ok(v) => pr(&v, true).map(Some),
            Err(e) if no_commits_between(&e) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

impl PrOpener for crate::GithubSync {
    fn ensure_draft_pr(
        &self,
        project: &Project,
        head: &str,
        base: &str,
    ) -> std::result::Result<Option<DraftPr>, String> {
        self.client().ensure_draft_pr(project, head, base)
    }
}

/// O modo do projeto neste ambiente: `(projeto, branch, base, estratégia)`.
struct Mode {
    project: Project,
    branch: String,
    base: String,
    strategy: Strategy,
}

/// Instrução ao agente da sessão autônoma (no system prompt, a cada início
/// e retomada), com o rótulo (PR ou MR) e o comando da forja do projeto.
pub fn agent_instruction(project: &Project, branch: &str, base: &str) -> String {
    let forge = project.forge.kind();
    let label = ct_core::change_label(forge);
    let create = match &project.forge {
        ct_core::ForgeConfig::Github => format!("gh pr create --base {branch}"),
        ct_core::ForgeConfig::Gitlab(g) => {
            let path = g.path.trim_matches('/');
            let path = if path.is_empty() {
                format!("{}/{}", project.gh_owner, project.gh_repo)
            } else {
                path.to_string()
            };
            format!(
                "{} mr create -R {path} -b {branch}",
                ct_core::glab_cli_prefix(g)
            )
        }
    };
    format!(
        "Modo autônomo (Control Tower): esta issue roda no modo autônomo do projeto. A branch \
desta sessão saiu de `{branch}`, a branch de integração: abra o {label} contra ela (`{create}`), \
nunca contra `{base}`. Quem leva `{branch}` para `{base}` é a pessoa, pelo {label} rascunho."
    )
}

/// Prompt inicial da sessão autônoma sem `prompt` no pedido (#191): o
/// template `autonomous` do projeto (vazio = o padrão do `ct-core`) com a
/// branch da sessão e a de integração já decididas.
pub(crate) fn autonomous_prompt(
    project: &Project,
    kind: ct_core::WorkItemKind,
    number: i64,
    title: &str,
    branch: &str,
    integration_branch: &str,
) -> String {
    ct_core::session_templates::AutonomousVars {
        tracker: project.tracker_ref(),
        forge: project.forge.kind(),
        kind,
        number,
        title,
        branch,
        integration_branch,
        protected: &project.protected_branches,
    }
    .render(project.session_templates.autonomous_template())
}

impl Sessions {
    fn autonomy_mode(&self, project_id: i64) -> Result<Mode> {
        let project = self.ledger().get_project(project_id)?;
        if !project.autonomy.enabled {
            return Err(SessionsError::Invalid(format!(
                "o modo autônomo de {}/{} está desligado no hub",
                project.gh_owner, project.gh_repo
            )));
        }
        if !project.autonomy_runner {
            return Err(SessionsError::Invalid(format!(
                "este ambiente não é o do modo autônomo de {}/{}",
                project.gh_owner, project.gh_repo
            )));
        }
        let branch = project.autonomy.branch().to_string();
        let base = project.autonomy.base(&project.base_branch).to_string();
        let strategy = match project.autonomy.strategy {
            AlignStrategy::Merge => Strategy::Merge,
            AlignStrategy::Rebase => Strategy::Rebase,
        };
        Ok(Mode {
            project,
            branch,
            base,
            strategy,
        })
    }

    /// Ligar o modo: cria a branch de integração a partir da base, se
    /// faltar, e abre o PR rascunho `branch → base` (`pr` = `None`: sem
    /// GitHub neste ambiente, o relato avisa). Grava e devolve o relato; a
    /// falha do git vira relato `error`.
    pub fn autonomy_setup(
        &self,
        project_id: i64,
        pr: Option<&dyn PrOpener>,
    ) -> Result<AutonomyReport> {
        let m = self.autonomy_mode(project_id)?;
        let _one = ALIGN.lock().unwrap_or_else(|e| e.into_inner());
        let report = match ensure_remote_branch(Path::new(&m.project.repo_path), &m.branch, &m.base)
        {
            Ok(created) => {
                if created {
                    tracing::info!(
                        "modo autônomo: {} criada a partir de {} em {}/{}",
                        m.branch,
                        m.base,
                        m.project.gh_owner,
                        m.project.gh_repo
                    );
                }
                self.report_ok(&m, pr)
            }
            Err(e) => self.report(&m, AutonomyState::Error, Some(e.to_string()), vec![], None),
        };
        self.ledger().record_autonomy_report(project_id, &report)?;
        Ok(report)
    }

    /// Alinhamento antes de iniciar uma issue: garante a branch, traz a base
    /// para ela e garante o PR rascunho. Conflito: grava o relato
    /// `conflict` (aviso no hub), avisa a UI e devolve
    /// [`SessionsError::AutonomyConflict`]. Outra falha do git: relato
    /// `error` e o erro.
    pub fn autonomy_align(
        &self,
        project_id: i64,
        pr: Option<&dyn PrOpener>,
    ) -> Result<AutonomyReport> {
        let m = self.autonomy_mode(project_id)?;
        let _one = ALIGN.lock().unwrap_or_else(|e| e.into_inner());
        let repo = Path::new(&m.project.repo_path);
        let aligned = ensure_remote_branch(repo, &m.branch, &m.base)
            .and_then(|_| align_branch(repo, &m.branch, &m.base, m.strategy));
        match aligned {
            Ok(AlignOutcome::UpToDate | AlignOutcome::Updated) => {
                let report = self.report_ok(&m, pr);
                self.ledger().record_autonomy_report(project_id, &report)?;
                Ok(report)
            }
            Ok(AlignOutcome::Conflict { files }) => {
                let warning = conflict_warning(&m.branch);
                let last = m.project.autonomy_report.as_ref();
                let report = AutonomyReport {
                    pr_pending: last.is_some_and(|r| r.pr_pending),
                    ..self.report(
                        &m,
                        AutonomyState::Conflict,
                        Some(warning.clone()),
                        files.clone(),
                        last.and_then(|r| r.pr_url.clone()),
                    )
                };
                {
                    let l = self.ledger();
                    if l.record_autonomy_report(project_id, &report)? {
                        // Um evento por conflito novo, no work item do
                        // projeto (#0), para o histórico.
                        let wi = l.ensure_work_item(
                            project_id,
                            crate::launch::PROJECT_WORK_ITEM,
                            ct_core::WorkItemKind::Issue,
                        )?;
                        l.append_event(NewEvent {
                            project_id,
                            work_item_id: Some(wi.id),
                            session_id: None,
                            kind: AUTONOMY_CONFLICT,
                            payload: json!({ "branch": m.branch, "base": m.base, "files": files }),
                        })?;
                    }
                }
                tracing::warn!(
                    "modo autônomo: {}/{}: {warning} ({})",
                    m.project.gh_owner,
                    m.project.gh_repo,
                    files.join(", ")
                );
                self.notify(
                    "alert",
                    json!({ "kind": AUTONOMY_CONFLICT, "project_id": project_id,
                            "error": warning, "files": files }),
                );
                Err(SessionsError::AutonomyConflict(format!(
                    "{warning}: a issue não começa até alguém resolver ({})",
                    files.join(", ")
                )))
            }
            Err(e) => {
                let report =
                    self.report(&m, AutonomyState::Error, Some(e.to_string()), vec![], None);
                self.ledger().record_autonomy_report(project_id, &report)?;
                Err(e.into())
            }
        }
    }

    /// Sessão autônoma da issue: alinha a branch de integração (conflito =
    /// a sessão não começa) e inicia com ela como base da worktree. O
    /// agente recebe a instrução de abrir o PR contra a branch de
    /// integração. `req.prompt` vazio (ou só espaços) = o ambiente monta o
    /// prompt com [`autonomous_prompt`] depois de decidir a branch (#191).
    pub fn start_autonomous(
        &self,
        req: StartRequest,
        pr: Option<&dyn PrOpener>,
    ) -> Result<Session> {
        // Rótulo de aprovação (#124) antes de alinhar a branch: a issue
        // recusada não mexe na wip. O agendador nunca manda o "mesmo assim".
        let project = self.ledger().get_project(req.project_id)?;
        self.check_approval(&project, &req)?;
        let report = self.autonomy_align(req.project_id, pr)?;
        self.start_with_base(req, Some((&report.branch, &report.base)))
    }

    /// Relato `ok`, com o PR rascunho garantido. A branch sem commits além
    /// da base (recém-criada) ainda não tem PR: `pr_pending` e a mensagem
    /// dizem que ele abre depois do primeiro merge, e o próximo alinhamento
    /// tenta de novo. Outra falha do PR não impede nada: vai na mensagem.
    fn report_ok(&self, m: &Mode, pr: Option<&dyn PrOpener>) -> AutonomyReport {
        // O PR rascunho é aberto pela API do GitHub: num projeto de outra
        // forja, o MR fica para a pessoa.
        let github = matches!(m.project.forge, ct_core::ForgeConfig::Github);
        let label = ct_core::change_label(m.project.forge.kind());
        let (url, message, pending) = match pr.filter(|_| github) {
            None => {
                let why = if github {
                    "sem GitHub neste ambiente"
                } else {
                    "fora do GitHub"
                };
                let message = format!(
                    "{why}: abra o {label} rascunho {} → {} à mão",
                    m.branch, m.base
                );
                (None, Some(message), false)
            }
            Some(p) => match p.ensure_draft_pr(&m.project, &m.branch, &m.base) {
                Ok(Some(d)) => {
                    if d.created {
                        tracing::info!("modo autônomo: PR rascunho aberto: {}", d.url);
                    }
                    (Some(d.url), None, false)
                }
                Ok(None) => (None, Some(draft_pr_pending_message(&m.branch)), true),
                Err(e) => (
                    None,
                    Some(format!(
                        "PR rascunho {} → {} não foi aberto: {e}",
                        m.branch, m.base
                    )),
                    false,
                ),
            },
        };
        AutonomyReport {
            pr_pending: pending,
            ..self.report(m, AutonomyState::Ok, message, vec![], url)
        }
    }

    fn report(
        &self,
        m: &Mode,
        state: AutonomyState,
        message: Option<String>,
        conflicts: Vec<String>,
        pr_url: Option<String>,
    ) -> AutonomyReport {
        AutonomyReport {
            state,
            branch: m.branch.clone(),
            base: m.base.clone(),
            message,
            conflicts,
            pr_url,
            pr_pending: false,
            at: now_ms(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{git_in, FakeClaude, TestEnv};
    use ct_core::AutonomyConfig;
    use std::sync::Mutex as StdMutex;

    /// PR falso: registra os pedidos.
    #[derive(Default)]
    struct FakePr(StdMutex<Vec<(String, String)>>);

    impl PrOpener for FakePr {
        fn ensure_draft_pr(
            &self,
            _project: &Project,
            head: &str,
            base: &str,
        ) -> std::result::Result<Option<DraftPr>, String> {
            let mut v = self.0.lock().unwrap();
            v.push((head.into(), base.into()));
            Ok(Some(DraftPr {
                number: 5,
                url: "https://example.com/acme/acme-web/pull/5".into(),
                created: v.len() == 1,
            }))
        }
    }

    /// Remoto `acme-web.git` com a `develop` do clone do ambiente, e o modo
    /// ligado (ou não) pelo pull do hub.
    fn env_com_remoto(on: bool, runner: bool) -> (TestEnv, std::path::PathBuf) {
        let env = TestEnv::new(FakeClaude::exit(0));
        let remote = env.tmp.path().join("acme-web.git");
        git_in(
            env.tmp.path(),
            &["init", "-q", "--bare", remote.to_str().unwrap()],
        );
        env.git(&["remote", "add", "origin", remote.to_str().unwrap()]);
        env.git(&["push", "-q", "origin", "develop"]);
        env.assign_config(1, |p| {
            p.autonomy = AutonomyConfig {
                enabled: on,
                env_id: Some("env-auto".into()),
                ..Default::default()
            };
            p.autonomy_runner = runner;
        });
        (env, remote)
    }

    fn ls_remote(env: &TestEnv, branch: &str) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&env.repo)
            .args(["ls-remote", "origin", &format!("refs/heads/{branch}")])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    /// Commit direto no remoto, por um clone à parte.
    fn commit_remoto(env: &TestEnv, remote: &Path, branch: &str, file: &str, text: &str) {
        let dir = env
            .tmp
            .path()
            .join(format!("outro-{}", branch.replace('/', "-")));
        if !dir.exists() {
            git_in(
                env.tmp.path(),
                &[
                    "clone",
                    "-q",
                    remote.to_str().unwrap(),
                    dir.to_str().unwrap(),
                ],
            );
            git_in(&dir, &["config", "user.email", "t@t"]);
            git_in(&dir, &["config", "user.name", "t"]);
        }
        git_in(&dir, &["fetch", "-q", "origin"]);
        git_in(
            &dir,
            &["checkout", "-q", "-B", branch, &format!("origin/{branch}")],
        );
        std::fs::write(dir.join(file), text).unwrap();
        git_in(&dir, &["add", "."]);
        git_in(&dir, &["commit", "-q", "-m", file]);
        git_in(&dir, &["push", "-q", "origin", branch]);
    }

    #[test]
    fn ligar_cria_a_branch_e_o_pr_rascunho() {
        let (env, _remote) = env_com_remoto(true, true);
        let pr = FakePr::default();
        let r = env
            .sessions
            .autonomy_setup(env.project_id, Some(&pr))
            .unwrap();
        assert_eq!(r.state, AutonomyState::Ok);
        assert_eq!(
            (r.branch.as_str(), r.base.as_str()),
            ("wip/autonoma", "develop")
        );
        assert_eq!(
            r.pr_url.as_deref(),
            Some("https://example.com/acme/acme-web/pull/5")
        );
        assert_eq!(ls_remote(&env, "wip/autonoma"), ls_remote(&env, "develop"));
        assert_eq!(
            pr.0.lock().unwrap().clone(),
            vec![("wip/autonoma".to_string(), "develop".to_string())]
        );
        let p = env.sessions.ledger().get_project(env.project_id).unwrap();
        assert_eq!(p.autonomy_report.unwrap().state, AutonomyState::Ok);
        // De novo: nada a criar.
        env.sessions
            .autonomy_setup(env.project_id, Some(&pr))
            .unwrap();
        // Sem GitHub: a branch sai, o relato avisa do PR.
        let r = env.sessions.autonomy_setup(env.project_id, None).unwrap();
        assert!(r.message.unwrap().contains("à mão"));
    }

    /// O PR rascunho pelo GitHub: abre uma vez (`draft: true`), depois acha
    /// o aberto.
    #[test]
    fn pr_rascunho_pela_api_do_github() {
        use ct_gh::fake::{fake_gh, FakeGitHub};
        use ct_gh::{GhClient, GhConfig};
        let (env, _remote) = env_com_remoto(true, true);
        let gh = FakeGitHub::start("tok");
        let bin = fake_gh(env.tmp.path(), "tok");
        let client = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT),
        );
        let list = ct_gh::sync::open_pulls_path("deep-ion-ai", "demo", "wip/autonoma", "develop");
        gh.set(&list, json!([]));
        gh.set(
            "/repos/deep-ion-ai/demo/pulls",
            json!({ "number": 9, "html_url": "https://example.com/pull/9" }),
        );
        let r = env
            .sessions
            .autonomy_setup(env.project_id, Some(&client))
            .unwrap();
        assert_eq!(r.pr_url.as_deref(), Some("https://example.com/pull/9"));
        let posted = gh.posted();
        assert_eq!(posted.len(), 1);
        assert_eq!(posted[0].1["draft"], true);
        assert_eq!(posted[0].1["head"], "wip/autonoma");
        assert_eq!(posted[0].1["base"], "develop");
        gh.set(
            &list,
            json!([{ "number": 9, "html_url": "https://example.com/pull/9" }]),
        );
        env.sessions
            .autonomy_setup(env.project_id, Some(&client))
            .unwrap();
        assert_eq!(gh.posted().len(), 1);
    }

    /// A branch recém-criada é igual à base: o GitHub recusa o PR com o
    /// `422` "No commits between". O relato fica `ok` com o PR pendente (não
    /// é erro), e o alinhamento seguinte, depois do primeiro merge na
    /// branch, abre o PR.
    #[test]
    fn pr_rascunho_pendente_ate_o_primeiro_merge_na_branch() {
        use ct_gh::fake::{fake_gh, FakeGitHub};
        use ct_gh::{GhClient, GhConfig};
        let (env, remote) = env_com_remoto(true, true);
        let gh = FakeGitHub::start("tok");
        let bin = fake_gh(env.tmp.path(), "tok");
        let client = GhClient::new(
            GhConfig::new(&gh.url(), bin.to_str().unwrap())
                .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT),
        );
        let list = ct_gh::sync::open_pulls_path("deep-ion-ai", "demo", "wip/autonoma", "develop");
        let create = "/repos/deep-ion-ai/demo/pulls";
        gh.set(&list, json!([]));
        gh.set(
            create,
            json!({ "number": 9, "html_url": "https://example.com/pull/9" }),
        );
        let sem_commits = || {
            json!({ "message": "Validation Failed", "errors": [
                { "resource": "PullRequest", "code": "custom",
                  "message": "No commits between develop and wip/autonoma" }
            ] })
        };
        gh.fail_next_with(create, 422, sem_commits());
        let r = env
            .sessions
            .autonomy_setup(env.project_id, Some(&client))
            .unwrap();
        assert_eq!(r.state, AutonomyState::Ok);
        assert!(r.pr_pending);
        assert_eq!(r.pr_url, None);
        assert_eq!(
            r.message.as_deref(),
            Some("o PR rascunho abre depois do primeiro merge na wip/autonoma")
        );
        let p = env.sessions.ledger().get_project(env.project_id).unwrap();
        assert!(p.autonomy_report.unwrap().pr_pending);
        // Alinhar antes da issue tenta de novo; ainda sem commits, segue
        // pendente.
        gh.fail_next_with(create, 422, sem_commits());
        let r = env
            .sessions
            .autonomy_align(env.project_id, Some(&client))
            .unwrap();
        assert!(r.pr_pending, "{r:?}");
        // Primeiro merge na branch: o alinhamento seguinte abre o PR.
        commit_remoto(&env, &remote, "wip/autonoma", "a.txt", "a");
        let r = env
            .sessions
            .autonomy_align(env.project_id, Some(&client))
            .unwrap();
        assert_eq!(r.state, AutonomyState::Ok);
        assert!(!r.pr_pending);
        assert_eq!(r.message, None);
        assert_eq!(r.pr_url.as_deref(), Some("https://example.com/pull/9"));
        // Uma tentativa por relato: as duas recusadas e a que abriu.
        let posts = gh
            .calls()
            .into_iter()
            .filter(|(m, p)| m == "POST" && p == create)
            .count();
        assert_eq!(posts, 3);
        // Outra recusa do GitHub não é pendência: vai na mensagem.
        gh.set(&list, json!([]));
        gh.fail_next(create, 500, &[]);
        let r = env
            .sessions
            .autonomy_align(env.project_id, Some(&client))
            .unwrap();
        assert!(!r.pr_pending);
        assert!(r.message.unwrap().contains("não foi aberto"));
    }

    /// Projeto fora do GitHub: o PR rascunho não passa pela API do GitHub;
    /// o relato pede o MR à mão.
    #[test]
    fn pr_rascunho_fora_do_github_fica_para_a_pessoa() {
        let (env, _remote) = env_com_remoto(true, true);
        env.assign_config(2, |p| {
            p.autonomy = AutonomyConfig {
                enabled: true,
                env_id: Some("env-auto".into()),
                ..Default::default()
            };
            p.autonomy_runner = true;
            p.forge = ct_core::ForgeConfig::Gitlab(ct_core::GitlabConfig {
                host: "gitlab.example.com".into(),
                path: "grupo/acme-web".into(),
                ..Default::default()
            });
        });
        let pr = FakePr::default();
        let r = env
            .sessions
            .autonomy_setup(env.project_id, Some(&pr))
            .unwrap();
        assert_eq!(r.state, AutonomyState::Ok);
        assert!(pr.0.lock().unwrap().is_empty());
        assert!(r.message.unwrap().contains("abra o MR rascunho"));
    }

    #[test]
    fn modo_desligado_ou_outro_ambiente_recusa() {
        for (on, runner) in [(false, false), (true, false)] {
            let (env, _remote) = env_com_remoto(on, runner);
            assert!(env.sessions.autonomy_setup(env.project_id, None).is_err());
            assert!(env
                .sessions
                .start_autonomous(env.request(123, "ACME-123 tela"), None)
                .is_err());
            assert_eq!(ls_remote(&env, "wip/autonoma"), "");
        }
    }

    #[test]
    fn sessao_autonoma_sai_da_wip_e_instrui_o_pr_contra_ela() {
        let (env, remote) = env_com_remoto(true, true);
        let pr = FakePr::default();
        env.sessions
            .autonomy_setup(env.project_id, Some(&pr))
            .unwrap();
        // A base andou: o alinhamento a traz para a wip antes de começar.
        commit_remoto(&env, &remote, "develop", "base.txt", "base\n");
        // Modelo e esforço da issue (#182) valem também na sessão autônoma.
        let mut req = env.request(123, "ACME-123 tela de login");
        req.agent = crate::AgentOptions {
            model: Some("opus".into()),
            effort: Some("high".into()),
            ..Default::default()
        };
        let s = env.sessions.start_autonomous(req, Some(&pr)).unwrap();
        let out = env.wait_fake_output(&s.id);
        assert!(out.contains("gh pr create --base wip/autonoma"), "{out}");
        assert!(out.contains("--model\nopus\n--effort\nhigh\n"), "{out}");
        let wi = env
            .sessions
            .ledger()
            .find_work_item(env.project_id, 123)
            .unwrap()
            .unwrap();
        let wt = Path::new(wi.worktree_path.as_deref().unwrap());
        // A worktree saiu da wip alinhada (tem o commit novo da base).
        assert!(wt.join("base.txt").exists());
        let ev = env
            .sessions
            .ledger()
            .list_events_of_kind(wi.id, AUTONOMY_SESSION)
            .unwrap();
        assert_eq!(ev[0].payload["branch"], "wip/autonoma");
        // A sessão comum da mesma issue (retomada ou nova) leva a instrução.
        env.wait_state(&s.id, ct_core::SessionState::Exited);
        let s2 = env
            .sessions
            .start(env.request(123, "ACME-123 tela de login"))
            .unwrap();
        assert!(env.wait_fake_output(&s2.id).contains("--base wip/autonoma"));
    }

    /// Sem prompt no pedido (#191): o padrão renderizado com a branch da
    /// sessão e a de integração; com sobrescrita no projeto, ela; só com
    /// espaços, o padrão; com prompt, o texto enviado.
    #[test]
    fn sessao_autonoma_sem_prompt_monta_o_template() {
        let (env, _remote) = env_com_remoto(true, true);
        env.sessions.autonomy_setup(env.project_id, None).unwrap();
        let start = |n: i64, prompt: &str| {
            let s = env
                .sessions
                .start_autonomous(
                    StartRequest {
                        prompt: prompt.into(),
                        ..env.request(n, "Tela de login")
                    },
                    None,
                )
                .unwrap();
            let out = env.wait_fake_output(&s.id);
            env.wait_state(&s.id, ct_core::SessionState::Exited);
            out
        };
        let out = start(12, "");
        assert!(
            out.contains("issue deep-ion-ai/demo#12 (Tela de login)"),
            "{out}"
        );
        assert!(out.contains("para wip/autonoma"), "{out}");
        assert!(out.contains("origin `autonoma`"), "{out}");
        let wi = env
            .sessions
            .ledger()
            .find_work_item(env.project_id, 12)
            .unwrap()
            .unwrap();
        let branch = wi.branch.unwrap();
        assert!(
            out.contains(&format!(
                "Branch de trabalho: {branch}, saída de wip/autonoma"
            )),
            "{out}"
        );

        // Sobrescrita do projeto.
        env.assign_config(2, |p| {
            p.autonomy = AutonomyConfig {
                enabled: true,
                env_id: Some("env-auto".into()),
                ..Default::default()
            };
            p.autonomy_runner = true;
            p.session_templates.autonomous = "Faça {{ref}} em {{branch}} → {{x}}".into();
        });
        let out = start(13, "  ");
        assert!(out.contains("Faça deep-ion-ai/demo#13 em "), "{out}");
        assert!(out.contains("→ {{x}}"), "{out}");
        assert!(!out.contains("Sessão autônoma do Control Tower"), "{out}");

        // Com prompt, vale o enviado.
        let out = start(14, "texto da UI");
        assert!(out.contains("texto da UI"), "{out}");
        assert!(!out.contains("Faça deep-ion-ai/demo#14"), "{out}");
    }

    /// A instrução do system prompt segue a forja: `gh pr create` no
    /// GitHub, `glab mr create` (e MR) no GitLab.
    #[test]
    fn instrucao_ao_agente_pela_forja() {
        let gh = Project {
            gh_owner: "acme".into(),
            gh_repo: "acme-web".into(),
            ..Default::default()
        };
        let i = agent_instruction(&gh, "wip/autonoma", "develop");
        assert!(
            i.contains("abra o PR contra ela (`gh pr create --base wip/autonoma`)"),
            "{i}"
        );
        assert!(i.contains("nunca contra `develop`"), "{i}");
        let gl = Project {
            forge: ct_core::ForgeConfig::Gitlab(ct_core::GitlabConfig {
                host: "gitlab.example.com".into(),
                path: "/acme/web/app/".into(),
                ..Default::default()
            }),
            ..gh.clone()
        };
        let i = agent_instruction(&gl, "wip/autonoma", "develop");
        assert!(
            i.contains("abra o MR contra ela (`GITLAB_HOST=gitlab.example.com glab mr create -R acme/web/app -b wip/autonoma`)"),
            "{i}"
        );
        assert!(i.contains("pelo MR rascunho"), "{i}");
        assert!(!i.contains("gh pr") && !i.contains(" PR "), "{i}");
    }

    /// Projeto Jira + GitLab: `{{ref}}` com a chave e o rótulo MR.
    #[test]
    fn sessao_autonoma_sem_prompt_no_jira() {
        let (env, _remote) = env_com_remoto(true, true);
        env.assign_config(2, |p| {
            p.autonomy = AutonomyConfig {
                enabled: true,
                env_id: Some("env-auto".into()),
                ..Default::default()
            };
            p.autonomy_runner = true;
            p.tracker = ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
                url: "https://jira.example.com".into(),
                project_key: "ACME".into(),
                jql: "project = ACME".into(),
                ..Default::default()
            });
            p.forge = ct_core::ForgeConfig::Gitlab(ct_core::GitlabConfig {
                host: "gitlab.example.com".into(),
                path: "deep-ion-ai/demo".into(),
                ..Default::default()
            });
        });
        env.sessions.autonomy_setup(env.project_id, None).unwrap();
        let s = env
            .sessions
            .start_autonomous(
                StartRequest {
                    prompt: String::new(),
                    ..env.request(123, "Tela de login")
                },
                None,
            )
            .unwrap();
        let out = env.wait_fake_output(&s.id);
        assert!(out.contains("issue ACME-123 (Tela de login)"), "{out}");
        assert!(out.contains("abra o MR de "), "{out}");
        assert!(
            out.contains("glab mr create -R deep-ion-ai/demo -b wip/autonoma"),
            "{out}"
        );
        assert!(!out.contains("gh pr create"), "{out}");
    }

    #[test]
    fn conflito_no_alinhamento_impede_o_inicio_e_gera_o_aviso() {
        let (env, remote) = env_com_remoto(true, true);
        env.sessions.autonomy_setup(env.project_id, None).unwrap();
        commit_remoto(&env, &remote, "wip/autonoma", "app.txt", "da wip\n");
        commit_remoto(&env, &remote, "develop", "app.txt", "da base\n");
        let e = env
            .sessions
            .start_autonomous(env.request(123, "ACME-123 tela"), None)
            .unwrap_err();
        assert!(matches!(e, SessionsError::AutonomyConflict(_)), "{e}");
        assert!(
            e.to_string()
                .contains("wip/autonoma em conflito com a base"),
            "{e}"
        );
        let l = env.sessions.ledger();
        let r = l
            .get_project(env.project_id)
            .unwrap()
            .autonomy_report
            .unwrap();
        assert_eq!(r.state, AutonomyState::Conflict);
        assert_eq!(r.conflicts, vec!["app.txt".to_string()]);
        // A issue não começou: nem work item.
        assert!(l.find_work_item(env.project_id, 123).unwrap().is_none());
        // O relato sobe ao hub na fotografia do projeto.
        let sobe = l.outbox_pending(1000).unwrap().into_iter().any(|e| {
            matches!(&e.change, ct_ledger::SyncChange::Project(p)
                if p.autonomy.as_ref().is_some_and(|a| a.state == AutonomyState::Conflict))
        });
        assert!(sobe);
    }

    #[test]
    fn modo_desligado_nao_muda_a_sessao_comum() {
        let (env, _remote) = env_com_remoto(false, false);
        let s = env
            .sessions
            .start(env.request(123, "ACME-123 tela"))
            .unwrap();
        let out = env.wait_fake_output(&s.id);
        assert!(!out.contains("Modo autônomo"), "{out}");
        assert_eq!(ls_remote(&env, "wip/autonoma"), "");
    }
}
