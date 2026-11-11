//! Modo autônomo no ambiente (épico #178, issue #181): a branch de
//! integração (`wip/autonoma`) e as sessões autônomas.
//!
//! - **Ligar o modo** ([`Sessions::autonomy_setup`]): o ambiente do modo
//!   cria a branch a partir da base, se faltar, e abre o PR rascunho
//!   `branch → base`. Roda na aplicação da configuração do hub.
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
use ct_core::autonomy::conflict_warning;
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
    fn ensure_draft_pr(
        &self,
        project: &Project,
        head: &str,
        base: &str,
    ) -> std::result::Result<DraftPr, String>;
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
    ) -> std::result::Result<DraftPr, String> {
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
            return pr(v, false);
        }
        let v = self
            .post_json(&format!("/repos/{o}/{r}/pulls"), &draft_pr_body(head, base))
            .map_err(|e| e.to_string())?;
        pr(&v, true)
    }
}

impl PrOpener for crate::GithubSync {
    fn ensure_draft_pr(
        &self,
        project: &Project,
        head: &str,
        base: &str,
    ) -> std::result::Result<DraftPr, String> {
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
/// e retomada).
pub fn agent_instruction(branch: &str, base: &str) -> String {
    format!(
        "Modo autônomo (Control Tower): esta issue roda no modo autônomo do projeto. A branch \
desta sessão saiu de `{branch}`, a branch de integração: abra o PR contra ela (`gh pr create \
--base {branch}`), nunca contra `{base}`. Quem leva `{branch}` para `{base}` é a pessoa, pelo PR \
rascunho."
    )
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
                let report = self.report(
                    &m,
                    AutonomyState::Conflict,
                    Some(warning.clone()),
                    files.clone(),
                    m.project
                        .autonomy_report
                        .as_ref()
                        .and_then(|r| r.pr_url.clone()),
                );
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
    /// integração.
    pub fn start_autonomous(
        &self,
        req: StartRequest,
        pr: Option<&dyn PrOpener>,
    ) -> Result<Session> {
        let report = self.autonomy_align(req.project_id, pr)?;
        self.start_with_base(req, Some((&report.branch, &report.base)))
    }

    /// Relato `ok`, com o PR rascunho garantido (a falha do PR não impede
    /// nada: vai na mensagem).
    fn report_ok(&self, m: &Mode, pr: Option<&dyn PrOpener>) -> AutonomyReport {
        let (url, message) = match pr {
            None => (
                None,
                Some(format!(
                    "sem GitHub neste ambiente: abra o PR rascunho {} → {} à mão",
                    m.branch, m.base
                )),
            ),
            Some(p) => match p.ensure_draft_pr(&m.project, &m.branch, &m.base) {
                Ok(d) => {
                    if d.created {
                        tracing::info!("modo autônomo: PR rascunho aberto: {}", d.url);
                    }
                    (Some(d.url), None)
                }
                Err(e) => (
                    None,
                    Some(format!(
                        "PR rascunho {} → {} não foi aberto: {e}",
                        m.branch, m.base
                    )),
                ),
            },
        };
        self.report(m, AutonomyState::Ok, message, vec![], url)
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
        ) -> std::result::Result<DraftPr, String> {
            let mut v = self.0.lock().unwrap();
            v.push((head.into(), base.into()));
            Ok(DraftPr {
                number: 5,
                url: "https://example.com/acme/acme-web/pull/5".into(),
                created: v.len() == 1,
            })
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
        let s = env
            .sessions
            .start_autonomous(env.request(123, "ACME-123 tela de login"), Some(&pr))
            .unwrap();
        let out = env.wait_fake_output(&s.id);
        assert!(out.contains("gh pr create --base wip/autonoma"), "{out}");
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
