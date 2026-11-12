//! Sessão de release (#204): iniciada só pela UI (`session.start_release`),
//! sem issue nem worktree, no clone principal do projeto. O work item é o
//! sentinela [`RELEASE_WORK_ITEM`]; o processo recebe
//! `CTOWER_SESSION_KIND=release` (perfil `release` do guarda-corpo) e o
//! token exclusivo da release como `GH_TOKEN`.
//!
//! O `ctower release …` fala com o daemon pelo token da sessão:
//! [`Sessions::release_context`] dá o repositório e a configuração, e
//! [`Sessions::release_event`] grava o desfecho de cada etapa no ledger. As
//! duas só respondem à sessão de release.

use crate::launch::{LaunchMode, LaunchSpec, PtySize};
use crate::{AgentOptions, Result, Sessions, SessionsError};
use ct_core::release::{ReleaseConfig, RELEASE_WORK_ITEM};
use ct_core::{ForgeKind, SessionState, TrackerKind, WorkItemKind};
use ct_ledger::{NewEvent, Project, Session, WorkItem};

use serde::Serialize;
use serde_json::Value;
use std::path::Path;

/// Tipo do evento do ledger com o desfecho de uma etapa do `ctower release`.
pub const RELEASE_STEP_EVENT: &str = "release_step";

/// Tamanho máximo do payload de um evento de etapa, em bytes.
pub const MAX_RELEASE_EVENT_BYTES: usize = 64 * 1024;

/// Limites da sessão de release, no system prompt.
pub(crate) const RELEASE_LIMITS: &str = "\
Sessão de release do Control Tower: você conduz a release do projeto até o fim, com autonomia \
para avançar e tratar falhas. O clone é só leitura. Merges e aprovações só pelo \
`ctower release <etapa>` (status, promote, merge-promotion, wait-release-pr, approve-runs, \
merge-release, wait-publish, merge-back), que confere as pré-condições e usa sempre merge commit \
preso ao SHA conferido: `git push`, `git merge`, `git commit`, `gh pr merge`, `gh pr review`, \
`gh release create|edit` e escritas pela `gh api` ficam bloqueados. Você pode reexecutar jobs \
que falharam (`gh run rerun <id> --failed`). Não corrija código nem CI: diante de conflito, check \
obrigatório vermelho ou prazo esgotado, pare e relate.";

/// O work item é o da sessão de release.
pub(crate) fn is_release_item(wi: &WorkItem) -> bool {
    wi.gh_number == RELEASE_WORK_ITEM
}

/// Contexto do `ctower release` (`POST /session/release/context`).
#[derive(Debug, Clone, Serialize)]
pub struct ReleaseContext {
    pub session_id: String,
    pub owner: String,
    pub repo: String,
    pub release: ReleaseConfig,
}

/// Recusa das rotas `/session/release/*`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReleaseApiError {
    /// Token da sessão ausente ou desconhecido.
    #[error("token da sessão ausente ou inválido")]
    Unauthorized,
    /// A sessão não é de release.
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Unavailable(String),
}

/// A release do projeto pode rodar: GitHub + GitHub e ligada.
fn check_project(project: &Project) -> Result<()> {
    if project.tracker.kind() != TrackerKind::Github || project.forge.kind() != ForgeKind::Github {
        return Err(SessionsError::Invalid(
            "sessão de release só em projetos GitHub + GitHub".into(),
        ));
    }
    if !project.release.enabled {
        return Err(SessionsError::Invalid(format!(
            "a release não está ligada no projeto {}/{} (Configurações › Release)",
            project.gh_owner, project.gh_repo
        )));
    }
    Ok(())
}

impl Sessions {
    /// Sessão de release do projeto: no clone principal, sem branch nem
    /// worktree. Recusa projeto sem a release ligada, sem o secret do token
    /// no cache e com outra sessão de release viva.
    pub fn start_release(
        &self,
        project_id: i64,
        prompt: &str,
        size: Option<PtySize>,
        agent: &AgentOptions,
    ) -> Result<Session> {
        agent.validate()?;
        let project = self.ledger().get_project(project_id)?;
        check_project(&project)?;
        if agent.remote_control {
            self.check_remote_control(&project)?;
        }
        if !Path::new(&project.repo_path).is_dir() {
            return Err(SessionsError::Invalid(format!(
                "clone do projeto não encontrado em {}",
                project.repo_path
            )));
        }
        let name = project.release.normalized().token_secret;
        if !self.project_has_secret(&project, &name) {
            return Err(SessionsError::Invalid(format!(
                "o secret {name} (token exclusivo da release) não está no cache do ambiente: \
                 cadastre-o no projeto, no hub"
            )));
        }
        // Conferência e criação juntas: dois cliques não abrem duas sessões.
        let _guard = self.inner.release_start.lock().unwrap();
        if let Some(s) = self.live_release_session(project_id)? {
            return Err(SessionsError::ReleaseBusy(format!(
                "já há uma sessão de release viva no projeto ({})",
                s.id
            )));
        }
        let wi =
            self.ledger()
                .ensure_work_item(project_id, RELEASE_WORK_ITEM, WorkItemKind::Issue)?;
        let tool_session_id = uuid::Uuid::new_v4().to_string();
        let remote_name = agent
            .remote_control
            .then(|| self.remote_name(&project, &wi, None));
        self.launch(
            &project,
            &wi,
            LaunchSpec {
                cwd: &project.repo_path,
                mode: LaunchMode::New(&tool_session_id),
                prompt,
                size: size.unwrap_or(PtySize::DEFAULT),
                agent,
                remote_name,
                title: "",
            },
        )
    }

    /// A sessão de release viva do projeto, se houver.
    pub fn live_release_session(&self, project_id: i64) -> Result<Option<Session>> {
        let l = self.ledger();
        let Some(wi) = l.find_work_item(project_id, RELEASE_WORK_ITEM)? else {
            return Ok(None);
        };
        Ok(l.live_sessions()?
            .into_iter()
            .find(|s| s.work_item_id == wi.id && s.state != SessionState::Exited))
    }

    /// O cache de secrets do ambiente tem `name` para o projeto.
    fn project_has_secret(&self, project: &Project, name: &str) -> bool {
        let key = crate::secrets::scope_of(project);
        match self.env_cache().load() {
            Ok(Some(s)) => s.resolve(key.as_deref()).contains_key(name),
            _ => false,
        }
    }

    /// Sessão, work item e projeto da sessão de release dona de `token`.
    fn release_of_token(
        &self,
        token: &str,
    ) -> std::result::Result<(Session, WorkItem, Project), ReleaseApiError> {
        let sid = self
            .session_of_token(token)
            .ok_or(ReleaseApiError::Unauthorized)?;
        let l = self.ledger();
        let s = l
            .get_session(&sid)
            .map_err(|e| ReleaseApiError::Unavailable(e.to_string()))?;
        let wi = l
            .get_work_item(s.work_item_id)
            .map_err(|e| ReleaseApiError::Unavailable(e.to_string()))?;
        let p = l
            .get_project(wi.project_id)
            .map_err(|e| ReleaseApiError::Unavailable(e.to_string()))?;
        if !is_release_item(&wi) {
            return Err(ReleaseApiError::Forbidden(
                "`ctower release` só roda na sessão de release do projeto".into(),
            ));
        }
        Ok((s, wi, p))
    }

    /// Contexto do `ctower release` para a sessão dona de `token`.
    pub fn release_context(
        &self,
        token: &str,
    ) -> std::result::Result<ReleaseContext, ReleaseApiError> {
        let (s, _, p) = self.release_of_token(token)?;
        if !p.release.enabled {
            return Err(ReleaseApiError::Forbidden(format!(
                "a release foi desligada no projeto {}/{}",
                p.gh_owner, p.gh_repo
            )));
        }
        Ok(ReleaseContext {
            session_id: s.id,
            owner: p.gh_owner,
            repo: p.gh_repo,
            release: p.release.normalized(),
        })
    }

    /// Grava o desfecho de uma etapa (`release_step`) no work item da
    /// sessão de release dona de `token`. `payload` é o JSON da etapa
    /// (`step`, `result`, ...).
    pub fn release_event(
        &self,
        token: &str,
        payload: &Value,
    ) -> std::result::Result<i64, ReleaseApiError> {
        let (s, wi, p) = self.release_of_token(token)?;
        let ok = payload
            .get("step")
            .and_then(Value::as_str)
            .is_some_and(|v| !v.is_empty())
            && payload.get("result").and_then(Value::as_str).is_some();
        if !ok {
            return Err(ReleaseApiError::Invalid(
                "evento de etapa sem step ou result".into(),
            ));
        }
        if payload.to_string().len() > MAX_RELEASE_EVENT_BYTES {
            return Err(ReleaseApiError::Invalid(format!(
                "evento de etapa maior que {} KiB",
                MAX_RELEASE_EVENT_BYTES / 1024
            )));
        }
        let id = self
            .ledger()
            .append_event(NewEvent {
                project_id: p.id,
                work_item_id: Some(wi.id),
                session_id: Some(&s.id),
                kind: RELEASE_STEP_EVENT,
                payload: payload.clone(),
            })
            .map_err(|e| ReleaseApiError::Unavailable(e.to_string()))?;
        tracing::info!(
            "release {}/{}: {} → {} (sessão {})",
            p.gh_owner,
            p.gh_repo,
            payload["step"].as_str().unwrap_or("?"),
            payload["result"].as_str().unwrap_or("?"),
            s.id
        );
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeClaude, TestEnv};
    use ct_secrets::Kind;
    use serde_json::json;

    const TOKEN: &str = "tok-release-exclusivo-1234";

    fn ligar(env: &TestEnv, rev: i64) {
        env.assign_config(rev, |p| {
            p.release = ReleaseConfig {
                enabled: true,
                ..Default::default()
            };
        });
    }

    fn cache(env: &TestEnv) {
        env.write_secrets_cache_kinds(&[
            (
                Some("deep-ion-ai/demo"),
                "CT_RELEASE_GH_TOKEN",
                Kind::Secret,
                TOKEN,
            ),
            (None, "GH_TOKEN", Kind::Secret, "tok-global-do-hub-9999"),
            (Some("deep-ion-ai/demo"), "T_OUTRO", Kind::Variable, "x"),
        ]);
    }

    fn token_of(env: &TestEnv, session_id: &str) -> String {
        env.wait_fake_output(session_id)
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{}=", crate::SESSION_TOKEN_ENV)))
            .expect("sessão sem token")
            .to_string()
    }

    fn start(env: &TestEnv) -> Result<Session> {
        env.sessions.start_release(
            env.project_id,
            "Conduza a release",
            None,
            &AgentOptions::default(),
        )
    }

    #[test]
    fn recusa_release_desligada_ou_sem_o_secret() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        env.assign_config(1, |_| {});
        cache(&env);
        let err = start(&env).unwrap_err().to_string();
        assert!(err.contains("não está ligada"), "{err}");

        ligar(&env, 2);
        env.write_secrets_cache(&[(Some("deep-ion-ai/demo"), "OUTRO", "x")]);
        let err = start(&env).unwrap_err().to_string();
        assert!(err.contains("CT_RELEASE_GH_TOKEN"), "{err}");
        assert!(!err.contains(TOKEN));
    }

    #[test]
    fn sessao_de_release_recebe_o_token_e_o_perfil() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        ligar(&env, 1);
        cache(&env);
        let s = start(&env).unwrap();
        let out = env.wait_fake_output(&s.id);
        assert!(out.contains("CTOWER_SESSION_KIND=release"), "{out}");
        assert!(out.contains(&format!("GH_TOKEN={TOKEN}")), "{out}");
        assert!(!out.contains("tok-global-do-hub"), "{out}");
        assert!(
            out.contains(&format!("CT_RELEASE_GH_TOKEN={TOKEN}")),
            "{out}"
        );
        assert!(!out.contains("CTOWER_ISSUE="), "{out}");
        // No clone principal, sem worktree.
        assert!(
            out.contains(&format!("PWD={}", env.repo.display())),
            "{out}"
        );
        assert!(out.contains("Sessão de release do Control Tower"), "{out}");
        let wi = env.sessions.ledger().get_work_item(s.work_item_id).unwrap();
        assert_eq!(wi.gh_number, RELEASE_WORK_ITEM);
        assert!(wi.worktree_path.is_none());

        // Só uma viva por projeto.
        let err = start(&env).unwrap_err();
        assert!(matches!(err, SessionsError::ReleaseBusy(_)), "{err}");

        // `ctower secret get` lê o token na sessão de release.
        let tok = token_of(&env, &s.id);
        assert_eq!(
            env.sessions
                .session_secret(&tok, "CT_RELEASE_GH_TOKEN")
                .unwrap(),
            TOKEN
        );
        env.sessions.kill(&s.id).unwrap();
        env.wait_state(&s.id, SessionState::Exited);
        // Encerrada, outra pode começar.
        let s2 = start(&env).unwrap();
        env.sessions.kill(&s2.id).unwrap();
    }

    #[test]
    fn sessao_de_issue_nao_recebe_o_token_da_release() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        ligar(&env, 1);
        cache(&env);
        let s = env.sessions.start(env.request(41, "Issue")).unwrap();
        let out = env.wait_fake_output(&s.id);
        assert!(!out.contains(TOKEN), "{out}");
        assert!(!out.contains("CT_RELEASE_GH_TOKEN"), "{out}");
        assert!(!out.contains("CTOWER_SESSION_KIND"), "{out}");
        // O GH_TOKEN do hub continua valendo nas outras sessões.
        assert!(out.contains("GH_TOKEN=tok-global-do-hub-9999"), "{out}");
        let tok = token_of(&env, &s.id);
        assert!(matches!(
            env.sessions.session_secret(&tok, "CT_RELEASE_GH_TOKEN"),
            Err(crate::SecretGetError::NotFound(_))
        ));
        // E não fala com as rotas da release.
        assert!(matches!(
            env.sessions.release_context(&tok),
            Err(ReleaseApiError::Forbidden(_))
        ));
        env.sessions.kill(&s.id).unwrap();
    }

    #[test]
    fn contexto_e_evento_da_etapa() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        ligar(&env, 1);
        cache(&env);
        let s = start(&env).unwrap();
        let tok = token_of(&env, &s.id);
        let ctx = env.sessions.release_context(&tok).unwrap();
        assert_eq!(
            (ctx.owner.as_str(), ctx.repo.as_str()),
            ("deep-ion-ai", "demo")
        );
        assert_eq!(ctx.release.target, "main");
        assert_eq!(ctx.session_id, s.id);

        let payload = json!({ "step": "promote", "result": "ok", "pr": 10, "sha": "d2" });
        env.sessions.release_event(&tok, &payload).unwrap();
        let evs = env
            .sessions
            .ledger()
            .list_events_of_kind(s.work_item_id, RELEASE_STEP_EVENT)
            .unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].payload, payload);
        assert_eq!(evs[0].session_id.as_deref(), Some(s.id.as_str()));

        assert!(matches!(
            env.sessions.release_event(&tok, &json!({ "x": 1 })),
            Err(ReleaseApiError::Invalid(_))
        ));
        assert!(matches!(
            env.sessions.release_context("token-errado"),
            Err(ReleaseApiError::Unauthorized)
        ));
        env.sessions.kill(&s.id).unwrap();
    }

    #[test]
    fn projeto_jira_gitlab_nao_tem_sessao_de_release() {
        let env = TestEnv::new(FakeClaude::sleep(30));
        env.assign_config(1, |p| {
            p.tracker = ct_core::TrackerConfig::Jira(ct_core::JiraConfig {
                url: "https://jira.example.com".into(),
                project_key: "ACME".into(),
                ..Default::default()
            });
            p.forge = ct_core::ForgeConfig::Gitlab(ct_core::GitlabConfig {
                host: "gitlab.example.com".into(),
                path: "acme/acme-web".into(),
                clone_url: "https://gitlab.example.com/acme/acme-web.git".into(),
                credential_cmd: String::new(),
            });
            // Mesmo que chegasse ligada (o hub recusa o campo nesses projetos).
            p.release = ReleaseConfig {
                enabled: true,
                ..Default::default()
            };
        });
        cache(&env);
        let err = start(&env).unwrap_err().to_string();
        assert!(err.contains("GitHub + GitHub"), "{err}");
    }
}
