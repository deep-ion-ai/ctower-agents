//! Projetos Jira + GitLab no hub (spec do Jira e GitLab §5, issue #348):
//! ambiente de sync e o espelho que só ele envia.
//!
//! O hub não sincroniza projeto Jira (não alcança o Jira nem o GitLab). Um
//! dos ambientes atribuídos, o **ambiente de sync** (`project.sync_env_id`),
//! manda pelo outbox as fotografias `issue_mirror`, `pr_mirror` e
//! `tracker_status`, que o hub aplica no `issue_cache`, `pr_cache` e
//! `sync_state`, as mesmas tabelas do sync do GitHub. Linhas de espelho de
//! qualquer outro ambiente são descartadas com ack: um escritor só por
//! projeto.
//!
//! A primeira atribuição de um projeto Jira sem ambiente de sync vira o
//! ambiente de sync, na mesma transação (`add_assignment`). Remover a
//! atribuição dele deixa o projeto sem ambiente de sync; o hub nunca escolhe
//! outro sozinho.

use super::{Hub, HubProject};
use crate::sync::project_key;
use crate::{LedgerError, PrRow, Result, SyncChange};
use ct_core::{now_ms, TrackerKind, TrackerReadiness};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Último `tracker_status` aceito do ambiente de sync (`hub.projects.list`).
/// O sync em si (tentativa, sucesso, erro) está no `sync` da visão.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubTrackerStatus {
    /// Ambiente que relatou (o de sync quando o relato chegou).
    pub env_id: String,
    pub env_name: String,
    /// Quando o hub recebeu (ms).
    pub received_at: i64,
    pub readiness: TrackerReadiness,
}

impl Hub {
    /// Aplica uma linha do espelho do rastreador (`issue_mirror`,
    /// `pr_mirror`, `tracker_status`) se `env` é o ambiente de sync do
    /// projeto Jira. `false` = descartada (outro ambiente, projeto GitHub ou
    /// sem ambiente de sync).
    pub(super) fn apply_mirror(&self, env: &str, change: &SyncChange) -> Result<bool> {
        let (owner, repo) = match change {
            SyncChange::IssueMirror(m) => (&m.owner, &m.repo),
            SyncChange::PrMirror(m) => (&m.owner, &m.repo),
            SyncChange::TrackerStatus(t) => (&t.owner, &t.repo),
            _ => unreachable!("só linhas de espelho"),
        };
        let row: Option<(i64, String, Option<String>)> = self
            .conn
            .query_row(
                "SELECT id, tracker_kind, sync_env_id FROM project
                 WHERE key = ?1 AND deleted_at IS NULL",
                [project_key(owner, repo)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let Some((pid, kind, sync_env)) = row else {
            return Ok(false);
        };
        if kind != TrackerKind::Jira.as_str() || sync_env.as_deref() != Some(env) {
            return Ok(false);
        }
        let now = now_ms();
        match change {
            SyncChange::IssueMirror(m) if m.removed => {
                // Saiu do JQL do projeto. Com work item no hub (de qualquer
                // ambiente), a issue fica, para a árvore não perdê-la.
                if self
                    .find_work_item(owner, repo, m.issue.gh_number)?
                    .is_none()
                {
                    self.conn.execute(
                        "DELETE FROM issue_cache WHERE project_id = ?1 AND gh_number = ?2",
                        params![pid, m.issue.gh_number],
                    )?;
                }
            }
            SyncChange::IssueMirror(m) => {
                self.gh().upsert_issue(pid, &m.issue, now)?;
            }
            SyncChange::PrMirror(m) => {
                // O work item costuma vir antes no outbox; se não veio, nasce
                // aqui, como numa sessão ou evento.
                let wi = self.ensure_work_item(owner, repo, m.gh_number)?;
                let pr = PrRow {
                    number: m.pr.number,
                    work_item_id: wi,
                    head: m.pr.head.clone(),
                    base: m.pr.base.clone(),
                    state: m.pr.state,
                    url: m.pr.url.clone(),
                    updated_at: m.pr.updated_at,
                    head_sha: m.pr.head_sha.clone(),
                };
                self.gh().upsert_pr(pid, &pr, now)?;
            }
            SyncChange::TrackerStatus(t) => {
                // A fotografia do ambiente substitui a anterior; um ciclo sem
                // sucesso ainda não visto (`last_ok_at` nulo) não apaga o
                // último sucesso conhecido.
                self.conn.execute(
                    "INSERT INTO sync_state (project_id, last_attempt_at, last_ok_at, last_error)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (project_id) DO UPDATE SET
                        last_attempt_at = excluded.last_attempt_at,
                        last_ok_at = coalesce(excluded.last_ok_at, sync_state.last_ok_at),
                        last_error = excluded.last_error",
                    params![pid, t.last_attempt_at, t.last_ok_at, t.last_error],
                )?;
                let readiness =
                    serde_json::to_string(&t.readiness).expect("TrackerReadiness serializa");
                self.conn.execute(
                    "INSERT INTO tracker_status (project_id, env_id, received_at, readiness)
                     VALUES (?1, ?2, ?3, ?4)
                     ON CONFLICT (project_id) DO UPDATE SET env_id = excluded.env_id,
                        received_at = excluded.received_at, readiness = excluded.readiness",
                    params![pid, env, now, readiness],
                )?;
            }
            _ => unreachable!(),
        }
        Ok(true)
    }

    /// Último `tracker_status` aceito do projeto.
    pub fn tracker_status(&self, project_id: i64) -> Result<Option<HubTrackerStatus>> {
        let row: Option<(String, Option<String>, i64, String)> = self
            .conn
            .query_row(
                "SELECT t.env_id, e.name, t.received_at, t.readiness FROM tracker_status t
                 LEFT JOIN env e ON e.env_id = t.env_id WHERE t.project_id = ?1",
                [project_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((env_id, name, received_at, readiness)) = row else {
            return Ok(None);
        };
        let readiness = serde_json::from_str(&readiness).map_err(|e| {
            LedgerError::Corrupt(format!("prontidão ilegível do projeto {project_id}: {e}"))
        })?;
        Ok(Some(HubTrackerStatus {
            env_name: name.unwrap_or_else(|| env_id.clone()),
            env_id,
            received_at,
            readiness,
        }))
    }

    /// `sync_env_id` = `env_id` no projeto, na transação de quem chama:
    /// confere que o projeto é Jira e que o ambiente está atribuído a ele.
    /// Devolve `false` se já era o ambiente de sync (nada muda).
    pub(super) fn set_sync_env_in_tx(
        &self,
        p: &HubProject,
        env_id: &str,
        env_label: &str,
        rev: i64,
    ) -> Result<bool> {
        if p.tracker.kind() != TrackerKind::Jira {
            return Err(LedgerError::Conflict(format!(
                "{}/{} é um projeto GitHub: o hub sincroniza, sem ambiente de sync",
                p.gh_owner, p.gh_repo
            )));
        }
        if p.sync_env_id.as_deref() == Some(env_id) {
            return Ok(false);
        }
        let assigned: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM assignment
                            WHERE project_id = ?1 AND env_id = ?2 AND removed_at IS NULL)",
            params![p.id, env_id],
            |r| r.get(0),
        )?;
        if !assigned {
            return Err(LedgerError::Conflict(format!(
                "o ambiente de sync precisa estar atribuído a {}/{}: atribua o projeto a \
                 {env_label} antes",
                p.gh_owner, p.gh_repo
            )));
        }
        self.ensure_env_row(env_id)?;
        self.conn.execute(
            "UPDATE project SET sync_env_id = ?2, rev = ?3 WHERE id = ?1",
            params![p.id, env_id, rev],
        )?;
        Ok(true)
    }

    /// Linha em `env` para a chave estrangeira de `sync_env_id`: o ambiente
    /// pode estar atribuído só pelo token preso a ele, sem ter enviado nada
    /// ainda. O nome vem do token.
    pub(super) fn ensure_env_row(&self, env_id: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO env (env_id, name, created_at)
             VALUES (?1, coalesce((SELECT name FROM env_token WHERE env_id = ?1), ?1), ?2)
             ON CONFLICT (env_id) DO NOTHING",
            params![env_id, now_ms()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        HealthFilter, HealthKind, HealthThresholds, HubProjectPatch, IssueMirrorChange, IssueRow,
        MirrorPr, NewHubProject, OutboxEntry, PrMirrorChange, ProjectProvider, Severity,
        TrackerStatusChange,
    };
    use ct_core::{
        ForgeConfig, GitlabConfig, JiraConfig, JiraFieldInfo, JiraReadiness, PrState,
        TrackerConfig, TrackerReadiness,
    };

    const OWNER: &str = "grupo/projeto";
    const REPO: &str = "app";
    const MIN: i64 = 60_000;

    fn jira() -> ProjectProvider {
        ProjectProvider {
            docs: Default::default(),
            tracker: TrackerConfig::Jira(JiraConfig {
                url: "https://jira.example.com".into(),
                project_key: "PROJ".into(),
                credential_cmd: "pass show jira | head -1".into(),
                type_map: ct_core::default_type_map(),
                active_sprint_only: true,
                ..Default::default()
            }),
            forge: ForgeConfig::Gitlab(GitlabConfig {
                host: "gitlab.example.com".into(),
                path: "grupo/projeto/app".into(),
                clone_url: "https://gitlab.example.com/grupo/projeto/app.git".into(),
                credential_cmd: "pass show gitlab | head -1".into(),
            }),
            branch_pattern: Some("{key}".into()),
            attribution: false,
        }
    }

    fn novo() -> NewHubProject {
        NewHubProject {
            owner: OWNER.into(),
            repo: REPO.into(),
            name: REPO.into(),
            base_branch: "develop".into(),
            protected_branches: vec!["develop".into()],
        }
    }

    /// Hub com o projeto Jira cadastrado e os ambientes `a` e `b` conhecidos
    /// (já falaram com o hub).
    fn hub_jira() -> Hub {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project_with(&novo(), &jira()).unwrap();
        for e in ["a", "b"] {
            hub.ingest(e, e, &[]).unwrap();
        }
        hub
    }

    fn entry(seq: i64, change: SyncChange) -> OutboxEntry {
        OutboxEntry {
            seq,
            created_at: 1,
            change,
        }
    }

    fn issue(n: i64, state: &str) -> SyncChange {
        SyncChange::IssueMirror(IssueMirrorChange {
            owner: OWNER.into(),
            repo: REPO.into(),
            issue: IssueRow {
                gh_number: n,
                title: format!("Issue {n}"),
                state: state.into(),
                labels: vec![],
                parent_number: Some(5014),
                has_sub_issues: false,
                url: format!("https://jira.example.com/browse/PROJ-{n}"),
                updated_at: 10,
                ..Default::default()
            },
            removed: false,
        })
    }

    fn mr(n: i64, iid: i64) -> SyncChange {
        SyncChange::PrMirror(PrMirrorChange {
            owner: OWNER.into(),
            repo: REPO.into(),
            gh_number: n,
            pr: MirrorPr {
                number: iid,
                head: format!("PROJ-{n}"),
                base: "develop".into(),
                state: PrState::Open,
                url: format!("https://gitlab.example.com/grupo/projeto/app/-/merge_requests/{iid}"),
                updated_at: 20,
                head_sha: Some("abc".into()),
            },
        })
    }

    fn status(ok: Option<i64>, error: Option<&str>) -> SyncChange {
        SyncChange::TrackerStatus(TrackerStatusChange {
            owner: OWNER.into(),
            repo: REPO.into(),
            last_attempt_at: 30,
            last_ok_at: ok,
            last_error: error.map(String::from),
            readiness: TrackerReadiness {
                owner: OWNER.into(),
                repo: REPO.into(),
                checked_at: 30,
                jira: JiraReadiness {
                    cli: Some("1.7.0".into()),
                    config_present: true,
                    credential_ok: error.is_none(),
                    custom_fields: vec![JiraFieldInfo {
                        id: "customfield_10099".into(),
                        name: "Responsável pela implementação".into(),
                        datatype: "user".into(),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
        })
    }

    fn project(hub: &Hub) -> HubProject {
        hub.find_project(OWNER, REPO).unwrap().unwrap()
    }

    #[test]
    fn cadastro_jira_gitlab_guarda_rastreador_forja_e_padrao() {
        let hub = Hub::open_in_memory().unwrap();
        let added = hub.add_project_with(&novo(), &jira()).unwrap();
        let p = added.project;
        assert_eq!(p.tracker, jira().tracker);
        assert_eq!(p.forge, jira().forge);
        assert_eq!(p.branch_pattern.as_deref(), Some("{key}"));
        assert!(!p.attribution);
        assert_eq!(p.sync_env_id, None);
        assert_eq!(p.item_ref(5450), "PROJ-5450");
        // Credenciais: só os comandos, como texto do cadastro.
        let cfg: String = hub
            .conn
            .query_row(
                "SELECT tracker_config FROM project WHERE id = ?1",
                [p.id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(cfg.contains("pass show jira"), "{cfg}");
    }

    #[test]
    fn cadastro_recusa_combinacao_e_padrao_invalidos() {
        let hub = Hub::open_in_memory().unwrap();
        let github_gitlab = ProjectProvider {
            tracker: TrackerConfig::Github,
            ..jira()
        };
        let jira_github = ProjectProvider {
            forge: ForgeConfig::Github,
            ..jira()
        };
        for pv in [github_gitlab, jira_github] {
            let err = hub.add_project_with(&novo(), &pv).unwrap_err().to_string();
            assert!(
                err.contains("combinação de rastreador e forja não suportada"),
                "{err}"
            );
        }
        for pat in ["{prefix}/{slug}", "{x}-{n}", "a..{n}"] {
            let pv = ProjectProvider {
                branch_pattern: Some(pat.into()),
                ..jira()
            };
            assert!(hub.add_project_with(&novo(), &pv).is_err(), "{pat}");
        }
        let mut sem_cmd = jira();
        if let TrackerConfig::Jira(j) = &mut sem_cmd.tracker {
            j.credential_cmd = " ".into();
        }
        let err = hub
            .add_project_with(&novo(), &sem_cmd)
            .unwrap_err()
            .to_string();
        assert!(err.contains("comando de credencial do Jira"), "{err}");
        // Fora do modo sprint ativa: JQL obrigatório e limite de 1 a 5000.
        let filtro = |jql: &str, max: u32| {
            let mut pv = jira();
            if let TrackerConfig::Jira(j) = &mut pv.tracker {
                j.active_sprint_only = false;
                j.jql = jql.into();
                j.max_issues = max;
            }
            pv
        };
        let err = filtro("  ", 100).validate().unwrap_err();
        assert!(err.contains("informe o JQL do sync"), "{err}");
        for max in [0, 5001] {
            let err = hub
                .add_project_with(&novo(), &filtro("labels = web", max))
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("limite de issues sincronizadas inválido"),
                "{err}"
            );
        }
        assert!(filtro("labels = web", 1).validate().is_ok());
        assert!(filtro("labels = web", 5000).validate().is_ok());
        // Cadastro anterior (sem JQL nem modo): a edição que não manda o
        // rastreador não confere o filtro.
        assert!(filtro("", 100).validate_base().is_ok());
        // Nada foi gravado.
        assert!(hub.find_project(OWNER, REPO).unwrap().is_none());
    }

    #[test]
    fn primeira_atribuicao_vira_o_ambiente_de_sync() {
        let hub = hub_jira();
        let a = hub.add_assignment(OWNER, REPO, "a", None).unwrap();
        assert!(a.sync_env);
        let b = hub.add_assignment(OWNER, REPO, "b", None).unwrap();
        assert!(!b.sync_env);
        assert_eq!(project(&hub).sync_env_id.as_deref(), Some("a"));
        let ep = Some(hub.config_epoch().unwrap());
        assert!(hub.env_config("a", None, ep).unwrap().projects.unwrap()[0].sync);
        assert!(!hub.env_config("b", None, ep).unwrap().projects.unwrap()[0].sync);

        // Projeto GitHub nunca ganha ambiente de sync.
        hub.add_project(&NewHubProject {
            owner: "deep-ion-ai".into(),
            repo: "demo".into(),
            ..novo()
        })
        .unwrap();
        assert!(
            !hub.add_assignment("deep-ion-ai", "demo", "a", None)
                .unwrap()
                .sync_env
        );
        let gh = hub.find_project("deep-ion-ai", "demo").unwrap().unwrap();
        assert_eq!(gh.sync_env_id, None);
        let err = hub
            .update_project(
                "deep-ion-ai",
                "demo",
                &HubProjectPatch {
                    sync_env: Some("a".into()),
                    ..Default::default()
                },
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("projeto GitHub"), "{err}");
    }

    #[test]
    fn ambiente_de_sync_so_entre_os_atribuidos_e_troca_incrementa_config_rev() {
        let hub = hub_jira();
        hub.add_assignment(OWNER, REPO, "a", None).unwrap();
        let sync_b = HubProjectPatch {
            sync_env: Some("b".into()),
            ..Default::default()
        };
        let err = hub
            .update_project(OWNER, REPO, &sync_b)
            .unwrap_err()
            .to_string();
        assert!(err.contains("precisa estar atribuído"), "{err}");
        assert_eq!(project(&hub).sync_env_id.as_deref(), Some("a"));

        hub.add_assignment(OWNER, REPO, "b", None).unwrap();
        let rev = hub.config_rev().unwrap();
        let p = hub.update_project(OWNER, REPO, &sync_b).unwrap();
        assert_eq!(p.sync_env_id.as_deref(), Some("b"));
        assert_eq!(hub.config_rev().unwrap(), rev + 1);
        // Os dois ambientes recebem a mudança no pull seguinte.
        let ep = Some(hub.config_epoch().unwrap());
        for (env, sync) in [("a", false), ("b", true)] {
            let c = hub.env_config(env, Some(rev), ep).unwrap();
            assert!(c.changed);
            let cp = &c.projects.unwrap()[0];
            assert_eq!(cp.sync, sync, "{env}");
            assert!(cp.rev > rev, "{env}");
        }

        // Remover a atribuição do ambiente de sync deixa o projeto sem; o hub
        // não escolhe o outro sozinho.
        hub.remove_assignment(OWNER, REPO, "b").unwrap();
        assert_eq!(project(&hub).sync_env_id, None);
        // Reatribuir um ambiente ao projeto sem ambiente de sync o elege.
        assert!(hub.add_assignment(OWNER, REPO, "b", None).unwrap().sync_env);
        assert_eq!(project(&hub).sync_env_id.as_deref(), Some("b"));
        // Remover outro ambiente não mexe no de sync.
        hub.remove_assignment(OWNER, REPO, "a").unwrap();
        assert_eq!(project(&hub).sync_env_id.as_deref(), Some("b"));
    }

    #[test]
    fn edicao_nao_muda_tipo_nem_repositorio() {
        let hub = hub_jira();
        let err = hub
            .update_project(
                OWNER,
                REPO,
                &HubProjectPatch {
                    tracker: Some(TrackerConfig::Github),
                    ..Default::default()
                },
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("rastreador não muda"), "{err}");
        let mut outro = jira().forge;
        if let ForgeConfig::Gitlab(g) = &mut outro {
            g.path = "grupo/projeto/outro".into();
        }
        let err = hub
            .update_project(
                OWNER,
                REPO,
                &HubProjectPatch {
                    forge: Some(outro),
                    ..Default::default()
                },
            )
            .unwrap_err()
            .to_string();
        assert!(err.contains("repositório do projeto não muda"), "{err}");
        // JQL, padrão (de volta ao do rastreador) e atribuição mudam.
        let mut t = jira().tracker;
        if let TrackerConfig::Jira(j) = &mut t {
            j.active_sprint_only = false;
            j.jql = "labels = web".into();
            j.max_issues = 250;
        }
        let p = hub
            .update_project(
                OWNER,
                REPO,
                &HubProjectPatch {
                    tracker: Some(t.clone()),
                    branch_pattern: Some(String::new()),
                    attribution: Some(true),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(p.tracker, t);
        assert_eq!((p.branch_pattern, p.attribution), (None, true));
    }

    #[test]
    fn espelho_so_do_ambiente_de_sync() {
        let hub = hub_jira();
        hub.add_assignment(OWNER, REPO, "a", None).unwrap();
        hub.add_assignment(OWNER, REPO, "b", None).unwrap();
        let p = project(&hub);
        let lote = vec![
            entry(1, issue(5450, "open")),
            entry(2, mr(5450, 77)),
            entry(3, status(Some(30), None)),
        ];

        // De quem não é o ambiente de sync: ack, nada aplicado.
        let r = hub.ingest("b", "b", &lote).unwrap();
        assert_eq!((r.applied, r.mirror_rejected), (0, 3));
        assert_eq!(r.mirror_rejected_projects, vec!["grupo/projeto/app"]);
        assert_eq!(r.last_seq, 3);
        assert!(hub.list_issues(p.id).unwrap().is_empty());
        assert!(hub.sync_state(p.id).unwrap().is_none());
        assert!(hub.tracker_status(p.id).unwrap().is_none());

        // Do ambiente de sync: issue, MR (com o work item) e o estado.
        let r = hub.ingest("a", "a", &lote).unwrap();
        assert_eq!((r.applied, r.mirror_rejected), (3, 0));
        let i = hub.get_issue(p.id, 5450).unwrap().unwrap();
        assert_eq!(
            (i.title.as_str(), i.parent_number),
            ("Issue 5450", Some(5014))
        );
        let wi = hub.find_work_item(OWNER, REPO, 5450).unwrap().unwrap();
        let prs = hub.prs_for_work_item(wi.id).unwrap();
        assert_eq!((prs[0].number, prs[0].head.as_str()), (77, "PROJ-5450"));
        assert_eq!(prs[0].head_sha.as_deref(), Some("abc"));
        let st = hub.sync_state(p.id).unwrap().unwrap();
        assert_eq!((st.last_ok_at, st.last_error), (Some(30), None));
        let ts = hub.tracker_status(p.id).unwrap().unwrap();
        assert_eq!((ts.env_id.as_str(), ts.env_name.as_str()), ("a", "a"));
        assert_eq!(ts.readiness.jira.cli.as_deref(), Some("1.7.0"));
        assert_eq!(ts.readiness.jira.custom_fields[0].id, "customfield_10099");
        let v = hub.project_views().unwrap();
        assert_eq!(v[0].tracker_status.as_ref().unwrap().env_id, "a");

        // Ciclo com erro: guarda o erro e o último sucesso conhecido.
        hub.ingest(
            "a",
            "a",
            &[entry(4, status(None, Some("o Jira recusou a senha (401)")))],
        )
        .unwrap();
        let st = hub.sync_state(p.id).unwrap().unwrap();
        assert_eq!(st.last_ok_at, Some(30));
        assert_eq!(
            st.last_error.as_deref(),
            Some("o Jira recusou a senha (401)")
        );

        // Trocado o ambiente de sync, o antigo deixa de valer.
        hub.update_project(
            OWNER,
            REPO,
            &HubProjectPatch {
                sync_env: Some("b".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let r = hub
            .ingest("a", "a", &[entry(5, issue(5450, "closed"))])
            .unwrap();
        assert_eq!(r.mirror_rejected, 1);
        assert_eq!(hub.get_issue(p.id, 5450).unwrap().unwrap().state, "open");
        let r = hub
            .ingest("b", "b", &[entry(6, issue(5450, "closed"))])
            .unwrap();
        assert_eq!(r.applied, 1);
        assert_eq!(hub.get_issue(p.id, 5450).unwrap().unwrap().state, "closed");
    }

    /// As sprints vêm no `issue_mirror` e saem na árvore do hub; a linha de
    /// um ambiente anterior (sem `sprints`) entra com a lista vazia.
    #[test]
    fn espelho_com_sprints_e_de_ambiente_anterior() {
        let hub = hub_jira();
        hub.add_assignment(OWNER, REPO, "a", None).unwrap();
        let sprint = ct_core::Sprint {
            id: 2273,
            name: "Sprint 48".into(),
            state: ct_core::SprintState::Active,
            board_id: Some(402),
            start_date: Some(1),
            end_date: Some(2),
            complete_date: None,
        };
        let mut com = issue(5450, "open");
        if let SyncChange::IssueMirror(m) = &mut com {
            m.issue.sprints = vec![sprint.clone()];
        }
        let mut v = serde_json::to_value(issue(5451, "open")).unwrap();
        let antiga = v
            .pointer_mut("/data/issue")
            .unwrap()
            .as_object_mut()
            .unwrap();
        assert!(antiga.remove("sprints").is_some());
        let sem: SyncChange = serde_json::from_value(v).unwrap();
        let r = hub
            .ingest("a", "a", &[entry(1, com), entry(2, sem)])
            .unwrap();
        assert_eq!(r.applied, 2);
        let p = project(&hub);
        assert_eq!(
            hub.get_issue(p.id, 5450).unwrap().unwrap().sprints,
            vec![sprint.clone()]
        );
        assert!(hub
            .get_issue(p.id, 5451)
            .unwrap()
            .unwrap()
            .sprints
            .is_empty());
        let tree = hub.tree(OWNER, REPO).unwrap().unwrap();
        let all: Vec<&IssueRow> = tree
            .roots
            .iter()
            .flat_map(|n| std::iter::once(&n.issue).chain(n.children.iter().map(|c| &c.issue)))
            .collect();
        let i = all.iter().find(|i| i.gh_number == 5450).unwrap();
        assert_eq!(i.sprints, vec![sprint]);
    }

    /// Os metadados vêm no `issue_mirror` e saem na árvore do hub; a linha
    /// de um ambiente anterior (sem `meta`) entra com a lista vazia.
    #[test]
    fn espelho_com_metadados_e_de_ambiente_anterior() {
        let hub = hub_jira();
        hub.add_assignment(OWNER, REPO, "a", None).unwrap();
        let meta = vec![
            crate::IssueMeta {
                field: "status".into(),
                label: "Situação".into(),
                value: "Em andamento".into(),
            },
            crate::IssueMeta {
                field: "assignee".into(),
                label: "Responsável".into(),
                value: "Fulano".into(),
            },
        ];
        let mut com = issue(5450, "open");
        if let SyncChange::IssueMirror(m) = &mut com {
            m.issue.meta = meta.clone();
        }
        let mut v = serde_json::to_value(issue(5451, "open")).unwrap();
        let antiga = v
            .pointer_mut("/data/issue")
            .unwrap()
            .as_object_mut()
            .unwrap();
        assert!(antiga.remove("meta").is_some());
        let sem: SyncChange = serde_json::from_value(v).unwrap();
        let r = hub
            .ingest("a", "a", &[entry(1, com), entry(2, sem)])
            .unwrap();
        assert_eq!(r.applied, 2);
        let p = project(&hub);
        assert_eq!(hub.get_issue(p.id, 5450).unwrap().unwrap().meta, meta);
        assert!(hub.get_issue(p.id, 5451).unwrap().unwrap().meta.is_empty());
        let tree = hub.tree(OWNER, REPO).unwrap().unwrap();
        let all: Vec<&IssueRow> = tree
            .roots
            .iter()
            .flat_map(|n| std::iter::once(&n.issue).chain(n.children.iter().map(|c| &c.issue)))
            .collect();
        let i = all.iter().find(|i| i.gh_number == 5450).unwrap();
        assert_eq!(i.meta, meta);
    }

    /// `issue_mirror` com `removed` (fora do JQL): o hub apaga a issue, a
    /// não ser que tenha work item (de qualquer ambiente).
    #[test]
    fn remocao_do_espelho_respeita_o_work_item_do_hub() {
        let hub = hub_jira();
        hub.add_assignment(OWNER, REPO, "a", None).unwrap();
        let p = project(&hub);
        hub.ingest(
            "a",
            "a",
            &[entry(1, issue(5450, "open")), entry(2, issue(5451, "open"))],
        )
        .unwrap();
        hub.ensure_work_item(OWNER, REPO, 5451).unwrap();
        let removed = |n: i64| {
            let mut c = issue(n, "open");
            if let SyncChange::IssueMirror(m) = &mut c {
                m.removed = true;
            }
            c
        };
        // O campo só vai no JSON quando é verdadeiro (o hub anterior ignora).
        let v = serde_json::to_value(issue(1, "open")).unwrap();
        assert!(v["data"].get("removed").is_none());
        assert_eq!(
            serde_json::to_value(removed(1)).unwrap()["data"]["removed"],
            true
        );
        let r = hub
            .ingest(
                "a",
                "a",
                &[entry(3, removed(5450)), entry(4, removed(5451))],
            )
            .unwrap();
        assert_eq!(r.applied, 2);
        assert!(hub.get_issue(p.id, 5450).unwrap().is_none());
        assert!(hub.get_issue(p.id, 5451).unwrap().is_some());
        // De outro ambiente, nada muda.
        hub.ingest("a", "a", &[entry(5, issue(5450, "open"))])
            .unwrap();
        hub.ingest("b", "b", &[entry(1, removed(5450))]).unwrap();
        assert!(hub.get_issue(p.id, 5450).unwrap().is_some());
    }

    #[test]
    fn espelho_de_projeto_github_e_descartado() {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project(&novo()).unwrap();
        hub.ingest("a", "a", &[]).unwrap();
        hub.add_assignment(OWNER, REPO, "a", None).unwrap();
        let r = hub.ingest("a", "a", &[entry(1, issue(1, "open"))]).unwrap();
        assert_eq!((r.applied, r.mirror_rejected), (0, 1));
    }

    #[test]
    fn exclusao_e_recadastro_limpam_o_espelho_e_o_ambiente_de_sync() {
        let hub = hub_jira();
        hub.add_assignment(OWNER, REPO, "a", None).unwrap();
        hub.ingest("a", "a", &[entry(1, status(Some(30), None))])
            .unwrap();
        let id = project(&hub).id;
        hub.delete_project(OWNER, REPO).unwrap();
        assert!(hub.tracker_status(id).unwrap().is_none());
        let sync: Option<String> = hub
            .conn
            .query_row("SELECT sync_env_id FROM project WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(sync, None);
        // Recadastro como GitHub não herda o Jira.
        let p = hub.add_project(&novo()).unwrap().project;
        assert_eq!(p.tracker, TrackerConfig::Github);
    }

    fn sinais(hub: &Hub, now: i64) -> Vec<crate::HealthSignal> {
        hub.health(
            &HealthThresholds::default(),
            60,
            &HealthFilter::default(),
            now,
        )
        .unwrap()
        .signals
        .into_iter()
        .filter(|s| s.repo.as_deref() == Some(REPO))
        .collect()
    }

    #[test]
    fn saude_avisa_sem_ambiente_de_sync_com_erro_e_com_sync_parado() {
        let hub = hub_jira();
        let created = project(&hub).created_at;
        // Sem ambiente de sync: aviso, mesmo recém-cadastrado, e nunca o do GitHub.
        let s = sinais(&hub, created + MIN);
        assert_eq!(s.len(), 1, "{s:?}");
        assert_eq!(
            (s[0].kind, s[0].severity),
            (HealthKind::RastreadorSemSync, Severity::Aviso)
        );
        assert!(s[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("sem ambiente de sync"));

        hub.add_assignment(OWNER, REPO, "a", None).unwrap();
        let t0 = ct_core::now_ms();
        let ok = |at: i64| {
            SyncChange::TrackerStatus(TrackerStatusChange {
                owner: OWNER.into(),
                repo: REPO.into(),
                last_attempt_at: at,
                last_ok_at: Some(at),
                last_error: None,
                readiness: TrackerReadiness::default(),
            })
        };
        hub.ingest("a", "a", &[entry(1, ok(t0))]).unwrap();
        // Sync em dia: nenhum sinal.
        assert!(sinais(&hub, t0 + MIN).is_empty());
        // Sync parado: aviso depois de 15 min e crítico depois de 24 h, com o
        // ambiente fora do ar citado.
        let s = sinais(&hub, t0 + 20 * MIN);
        assert_eq!(s[0].severity, Severity::Aviso);
        let d = s[0].detail.as_deref().unwrap();
        assert!(
            d.contains("ambiente de sync a") && d.contains("sem falar com o hub"),
            "{d}"
        );
        assert_eq!(s[0].env.as_deref(), Some("a"));
        assert_eq!(
            sinais(&hub, t0 + 25 * 60 * MIN)[0].severity,
            Severity::Critico
        );

        // Erro no último ciclo, mesmo recente: aviso com o motivo.
        hub.ingest(
            "a",
            "a",
            &[entry(
                2,
                status(None, Some("comando de credencial do Jira falhou (saída 2)")),
            )],
        )
        .unwrap();
        hub.conn
            .execute("UPDATE sync_state SET last_ok_at = ?1", [t0])
            .unwrap();
        let s = sinais(&hub, t0 + MIN);
        assert_eq!(
            (s[0].kind, s[0].severity),
            (HealthKind::RastreadorSemSync, Severity::Aviso)
        );
        assert!(s[0].detail.as_deref().unwrap().contains("saída 2"), "{s:?}");

        // O ambiente de sync some (atribuição removida): volta o aviso.
        hub.remove_assignment(OWNER, REPO, "a").unwrap();
        let s = sinais(&hub, t0 + MIN);
        assert!(s[0]
            .detail
            .as_deref()
            .unwrap()
            .contains("sem ambiente de sync"));
    }
}
