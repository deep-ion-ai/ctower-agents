//! Modo autônomo no hub (épico #178, issue #181): a marca do ambiente que
//! pode rodar sessões autônomas e a validação do modo no cadastro do
//! projeto. O modo em si vai aos ambientes pelo `config_rev`
//! (`ConfigProject.autonomy`), e o relato da branch de integração volta na
//! fotografia do projeto (`ProjectChange.autonomy`).

use super::{Hub, HubProject};
use crate::{LedgerError, Result};
use ct_core::AutonomyConfig;
use rusqlite::{params, OptionalExtension};

impl Hub {
    /// Marca ou desmarca o ambiente como autônomo (`hub.envs.set_autonomous`).
    /// Desmarcar um ambiente que roda o modo ligado de algum projeto é
    /// recusado: desligue o modo (ou troque o ambiente) antes. Devolve o
    /// `env_id`.
    pub fn set_env_autonomous(&self, env: &str, autonomous: bool) -> Result<String> {
        let env_id = self.known_env(env)?;
        if !autonomous {
            let using = self.autonomy_projects_of(&env_id)?;
            if !using.is_empty() {
                return Err(LedgerError::Conflict(format!(
                    "o ambiente roda o modo autônomo de {}: desligue o modo ou troque o ambiente \
                     antes",
                    using.join(", ")
                )));
            }
        }
        self.conn.execute(
            "UPDATE env SET autonomous = ?2 WHERE env_id = ?1",
            params![env_id, autonomous],
        )?;
        Ok(env_id)
    }

    /// `env_id` de um ambiente que já enviou ao hub, por `env_id` ou nome.
    fn known_env(&self, env: &str) -> Result<String> {
        let env = env.trim();
        let ids: Vec<String> = {
            let mut stmt = self.conn.prepare(
                "SELECT env_id FROM env WHERE (env_id = ?1 OR name = ?1)
                   AND last_seen_at IS NOT NULL
                 ORDER BY env_id = ?1 DESC, env_id",
            )?;
            let rows = stmt.query_map([env], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        match ids.as_slice() {
            [] => Err(LedgerError::NotFound(
                "ambiente",
                format!("{env} não é conhecido do hub (ele precisa ter enviado ao hub uma vez)"),
            )),
            [id, ..] if id == env => Ok(id.clone()),
            [id] => Ok(id.clone()),
            _ => Err(LedgerError::Conflict(format!(
                "há mais de um ambiente chamado {env}: use o env_id ({})",
                ids.join(", ")
            ))),
        }
    }

    /// `owner/repo` dos projetos com o modo ligado no ambiente.
    fn autonomy_projects_of(&self, env_id: &str) -> Result<Vec<String>> {
        Ok(self
            .list_projects()?
            .into_iter()
            .filter(|p| p.autonomy.enabled && p.autonomy.env_id.as_deref() == Some(env_id))
            .map(|p| format!("{}/{}", p.gh_owner, p.gh_repo))
            .collect())
    }

    /// O ambiente está marcado como autônomo.
    pub fn env_is_autonomous(&self, env_id: &str) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT autonomous FROM env WHERE env_id = ?1",
                [env_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    /// Normaliza e confere o modo autônomo pedido para o projeto, com a
    /// base e as protegidas que o projeto terá depois da edição. O ambiente
    /// (`env_id` ou nome) vira o `env_id`. Ligado, exige projeto GitHub e um
    /// ambiente marcado como autônomo e atribuído ao projeto.
    pub(super) fn checked_autonomy(
        &self,
        cur: &HubProject,
        cfg: &AutonomyConfig,
        base_branch: &str,
        protected: &[String],
    ) -> Result<AutonomyConfig> {
        let refuse = |m: String| Err(LedgerError::Conflict(m));
        let mut cfg = cfg.normalized(base_branch);
        if let Some(env) = cfg.env_id.clone() {
            cfg.env_id = Some(self.known_env(&env)?);
        }
        cfg.validate(base_branch, protected)
            .map_err(LedgerError::Conflict)?;
        if !cfg.enabled {
            return Ok(cfg);
        }
        if cur.forge.gitlab().is_some() {
            return refuse(
                "modo autônomo: por enquanto só em projetos do GitHub (a branch de integração e \
                 o PR rascunho usam a API do GitHub)"
                    .into(),
            );
        }
        let env_id = cfg.env_id.clone().expect("validado: ligado tem ambiente");
        if !self.env_is_autonomous(&env_id)? {
            return refuse(format!(
                "modo autônomo: o ambiente {env_id} não está marcado como autônomo (Hub › \
                 Ambientes)"
            ));
        }
        let assigned = self
            .project_assignments(cur.id)?
            .iter()
            .any(|a| a.env_id == env_id);
        if !assigned {
            return refuse(format!(
                "modo autônomo: atribua o projeto ao ambiente {env_id} antes de ligar o modo"
            ));
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use crate::{Hub, HubProjectPatch, NewHubProject, OutboxEntry, ProjectChange, SyncChange};
    use ct_core::{AutonomyConfig, AutonomyReport, AutonomyState};

    fn hub_com_ambientes() -> Hub {
        let hub = Hub::open_in_memory().unwrap();
        for env in ["env-auto", "env-mac"] {
            hub.ingest(env, env, &[]).unwrap();
        }
        hub.add_project(&NewHubProject {
            owner: "acme".into(),
            repo: "acme-web".into(),
            name: "acme-web".into(),
            base_branch: "develop".into(),
            protected_branches: vec!["develop".into(), "main".into()],
        })
        .unwrap();
        hub
    }

    fn ligar(hub: &Hub, env: &str) -> crate::Result<crate::HubProject> {
        hub.update_project(
            "acme",
            "acme-web",
            &HubProjectPatch {
                autonomy: Some(AutonomyConfig {
                    enabled: true,
                    env_id: Some(env.into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
    }

    #[test]
    fn ligar_exige_ambiente_autonomo_e_atribuido() {
        let hub = hub_com_ambientes();
        let e = ligar(&hub, "env-auto").unwrap_err().to_string();
        assert!(e.contains("não está marcado como autônomo"), "{e}");
        hub.set_env_autonomous("env-auto", true).unwrap();
        let e = ligar(&hub, "env-auto").unwrap_err().to_string();
        assert!(e.contains("atribua o projeto"), "{e}");
        hub.add_assignment("acme", "acme-web", "env-auto", None)
            .unwrap();
        hub.add_assignment("acme", "acme-web", "env-mac", None)
            .unwrap();
        let p = ligar(&hub, "env-auto").unwrap();
        assert!(p.autonomy.is_on());
        assert_eq!(p.autonomy.branch(), "wip/autonoma");
        assert_eq!(p.autonomy.base(&p.base_branch), "develop");
        assert!(hub.list_envs().unwrap().iter().any(|e| e.autonomous));

        // O pull leva o modo aos dois; só o do modo o roda.
        let auto = hub.env_config("env-auto", None, None).unwrap();
        let mac = hub.env_config("env-mac", None, None).unwrap();
        let (a, m) = (&auto.projects.unwrap()[0], &mac.projects.unwrap()[0]);
        assert!(a.autonomy_runner && !m.autonomy_runner);
        assert_eq!(a.autonomy, m.autonomy);

        // Desmarcar o ambiente em uso é recusado.
        let e = hub
            .set_env_autonomous("env-auto", false)
            .unwrap_err()
            .to_string();
        assert!(e.contains("acme/acme-web"), "{e}");

        // Tirar a atribuição desliga o modo (as escolhas ficam).
        hub.remove_assignment("acme", "acme-web", "env-auto")
            .unwrap();
        let p = hub.find_project("acme", "acme-web").unwrap().unwrap();
        assert!(!p.autonomy.enabled);
        assert_eq!(p.autonomy.env_id.as_deref(), Some("env-auto"));
        hub.set_env_autonomous("env-auto", false).unwrap();
    }

    #[test]
    fn branch_protegida_ou_igual_a_base_e_recusada() {
        let hub = hub_com_ambientes();
        for (branch, base) in [("main", ""), ("develop", ""), ("wip/x", "wip/x")] {
            let e = hub
                .update_project(
                    "acme",
                    "acme-web",
                    &HubProjectPatch {
                        autonomy: Some(AutonomyConfig {
                            branch: branch.into(),
                            base_branch: base.into(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                )
                .unwrap_err();
            assert!(e.to_string().contains("modo autônomo"), "{e}");
        }
    }

    #[test]
    fn relato_do_ambiente_do_modo_aparece_no_projeto() {
        let hub = hub_com_ambientes();
        hub.set_env_autonomous("env-auto", true).unwrap();
        hub.add_assignment("acme", "acme-web", "env-auto", None)
            .unwrap();
        ligar(&hub, "env-auto").unwrap();
        let report = |state: AutonomyState, seq: i64| OutboxEntry {
            seq,
            created_at: 1,
            change: SyncChange::Project(ProjectChange {
                owner: "acme".into(),
                repo: "acme-web".into(),
                name: "acme-web".into(),
                repo_path: "/tmp/acme-web".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
                created_at: 1,
                hlc: seq,
                state: None,
                error: None,
                applied_rev: Some(1),
                tracker: Default::default(),
                forge: Default::default(),
                branch_pattern: None,
                attribution: true,
                autonomy: Some(AutonomyReport {
                    state,
                    branch: "wip/autonoma".into(),
                    base: "develop".into(),
                    message: Some(ct_core::autonomy::conflict_warning("wip/autonoma")),
                    conflicts: vec!["app.txt".into()],
                    pr_url: None,
                    at: 1,
                }),
            }),
        };
        // Relato de outro ambiente não conta.
        hub.ingest("env-mac", "env-mac", &[report(AutonomyState::Conflict, 1)])
            .unwrap();
        let p = hub.find_project("acme", "acme-web").unwrap().unwrap();
        assert_eq!(p.autonomy_report, None);
        hub.ingest(
            "env-auto",
            "env-auto",
            &[report(AutonomyState::Conflict, 2)],
        )
        .unwrap();
        let p = hub.find_project("acme", "acme-web").unwrap().unwrap();
        let r = p.autonomy_report.unwrap();
        assert_eq!(r.state, AutonomyState::Conflict);
        assert_eq!(
            r.message.as_deref(),
            Some("wip/autonoma em conflito com a base")
        );
    }
}
