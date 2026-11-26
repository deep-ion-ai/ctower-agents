//! Limite de trabalho em andamento por projeto (issue #126, épico #178). O
//! limite fica no cadastro do projeto (`HubProject.wip_limits`); o uso é
//! contado aqui, com o que o hub sabe de todos os ambientes:
//!
//! - sessões de implementação vivas (`starting`, `running`, `idle`) em
//!   issues e épicos do projeto, de qualquer ambiente; a sessão do projeto
//!   (planejamento, `#0`) não conta;
//! - PRs abertos do espelho (`pr_cache`) ligados a work items do projeto.
//!
//! [`Hub::project_wip`] e [`Hub::wip_usage`] são a consulta do agendador do
//! piloto automático (#183): `has_room()` diz se cabe mais uma sessão.

use super::Hub;
use crate::{LedgerError, Result};
use ct_core::{WipLimits, WipUsage};

impl Hub {
    /// Uso e limites do projeto `project_id` agora.
    pub fn project_wip(&self, project_id: i64) -> Result<WipUsage> {
        let (limits, sessions, prs): (String, i64, i64) = self.conn.query_row(
            "SELECT p.wip_limits,
                    (SELECT count(*) FROM session s JOIN work_item w ON w.id = s.work_item_id
                      WHERE w.project_id = p.id AND w.gh_number > 0
                        AND s.state IN ('starting', 'running', 'idle')),
                    (SELECT count(*) FROM pr_cache c
                      WHERE c.project_id = p.id AND c.state = 'open')
             FROM project p WHERE p.id = ?1",
            [project_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(WipUsage {
            sessions: sessions.max(0) as u32,
            prs: prs.max(0) as u32,
            // Um JSON ilegível vale como sem limite, como no resto do cadastro.
            limits: serde_json::from_str(&limits).unwrap_or_default(),
        })
    }

    /// Uso e limites de `owner/repo`; `None` se o projeto não está no hub.
    pub fn wip_usage(&self, owner: &str, repo: &str) -> Result<Option<WipUsage>> {
        match self.find_project(owner, repo)? {
            Some(p) => Ok(Some(self.project_wip(p.id)?)),
            None => Ok(None),
        }
    }

    /// Grava os limites do projeto (dentro da transação de
    /// `update_project`). Limite fora de 1..=`WIP_LIMIT_MAX` é recusado.
    pub(super) fn set_wip_limits_in_tx(&self, key: &str, w: &WipLimits) -> Result<()> {
        w.validate().map_err(LedgerError::Conflict)?;
        self.conn.execute(
            "UPDATE project SET wip_limits = ?2 WHERE key = ?1 AND deleted_at IS NULL",
            rusqlite::params![key, w.config_json()],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{Hub, HubProjectPatch, NewHubProject};
    use ct_core::WipLimits;
    use rusqlite::params;

    fn hub_acme() -> (Hub, i64) {
        let hub = Hub::open_in_memory().unwrap();
        for env in ["env-a", "env-b"] {
            hub.ingest(env, env, &[]).unwrap();
        }
        let p = hub
            .add_project(&NewHubProject {
                owner: "acme".into(),
                repo: "acme-web".into(),
                name: "acme-web".into(),
                base_branch: "develop".into(),
                protected_branches: vec!["develop".into(), "main".into()],
            })
            .unwrap();
        (hub, p.project.id)
    }

    fn sessao(hub: &Hub, id: &str, env: &str, number: i64, state: &str) {
        let wi = hub.ensure_work_item("acme", "acme-web", number).unwrap();
        hub.conn
            .execute(
                "INSERT INTO session (id, env_id, work_item_id, tool, cwd, state, started_at,
                                      last_event_at)
                 VALUES (?1, ?2, ?3, 'claude', '/tmp/acme-web', ?4, 1, 1)",
                params![id, env, wi, state],
            )
            .unwrap();
    }

    fn pr(hub: &Hub, pid: i64, number: i64, issue: i64, state: &str) {
        let wi = hub.ensure_work_item("acme", "acme-web", issue).unwrap();
        hub.conn
            .execute(
                "INSERT INTO pr_cache (project_id, number, work_item_id, head, base, state, url,
                                       updated_at, synced_at)
                 VALUES (?1, ?2, ?3, 'feat/x', 'develop', ?4, 'https://example.test/pr', 1, 1)",
                params![pid, number, wi, state],
            )
            .unwrap();
    }

    fn limitar(hub: &Hub, w: WipLimits) -> crate::Result<crate::HubProject> {
        hub.update_project(
            "acme",
            "acme-web",
            &HubProjectPatch {
                wip_limits: Some(w),
                ..Default::default()
            },
        )
    }

    #[test]
    fn conta_sessoes_vivas_de_todos_os_ambientes_e_prs_abertos() {
        let (hub, pid) = hub_acme();
        let u = hub.wip_usage("acme", "acme-web").unwrap().unwrap();
        assert_eq!((u.sessions, u.prs), (0, 0));
        assert!(u.limits.is_empty() && u.has_room());

        sessao(&hub, "s1", "env-a", 123, "running");
        sessao(&hub, "s2", "env-b", 124, "idle");
        sessao(&hub, "s3", "env-a", 125, "exited");
        // Sessão do projeto (planejamento) não é de implementação.
        sessao(&hub, "s4", "env-a", 0, "running");
        pr(&hub, pid, 10, 123, "open");
        pr(&hub, pid, 11, 126, "merged");

        let p = limitar(
            &hub,
            WipLimits {
                max_sessions: Some(2),
                max_prs: Some(3),
            },
        )
        .unwrap();
        assert_eq!(p.wip_limits.max_sessions, Some(2));
        let u = hub.project_wip(pid).unwrap();
        assert_eq!((u.sessions, u.prs), (2, 1));
        assert!(!u.has_room() && u.sessions_full() && !u.prs_full());
        assert_eq!(
            u.reason().unwrap(),
            "limite de trabalho em andamento (2 de 2 sessões)"
        );
        // A visão do projeto leva o uso à UI.
        let v = hub.project_views().unwrap();
        assert_eq!(v[0].wip, Some(u));
        // E o overview do gestor, com o motivo.
        let o = hub
            .agent_overview(&crate::HealthThresholds::default(), 0, ct_core::now_ms())
            .unwrap();
        let a = o.projects.iter().find(|p| p.repo == "acme-web").unwrap();
        assert_eq!(a.wip, Some(u));
        assert_eq!(
            a.wip_limit_reached.as_deref(),
            Some("limite de trabalho em andamento (2 de 2 sessões)")
        );

        // `{}` volta a sem limite; outra edição não mexe no limite.
        limitar(&hub, WipLimits::default()).unwrap();
        assert!(hub.project_wip(pid).unwrap().has_room());
        let o = hub
            .agent_overview(&crate::HealthThresholds::default(), 0, ct_core::now_ms())
            .unwrap();
        assert_eq!(o.projects[0].wip_limit_reached, None);
        let json = serde_json::to_value(&o.projects[0]).unwrap();
        assert!(json.get("wip_limit_reached").is_none());
    }

    #[test]
    fn limite_invalido_e_recusado_sem_mudar_nada() {
        let (hub, pid) = hub_acme();
        let e = limitar(
            &hub,
            WipLimits {
                max_sessions: Some(0),
                max_prs: None,
            },
        )
        .unwrap_err();
        assert!(e.to_string().contains("sessões simultâneas"), "{e}");
        assert!(hub.project_wip(pid).unwrap().limits.is_empty());
        assert!(hub.wip_usage("acme", "outro").unwrap().is_none());
    }
}
