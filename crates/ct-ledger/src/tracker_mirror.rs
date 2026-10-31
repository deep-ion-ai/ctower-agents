//! Espelho do rastreador enviado ao hub pelo ambiente de sync de um projeto
//! Jira (spec do Jira e GitLab §5.3, issues #349 e #350): `issue_mirror`
//! com as issues que mudaram, `pr_mirror` com os MRs que mudaram e
//! `tracker_status` ao fim de cada ciclo. Nada entra no
//! outbox sem `sync_role = 1`: os outros ambientes atribuídos sincronizam
//! só para uso local.

use crate::{
    IssueMirrorChange, IssueRow, Ledger, MirrorPr, PrMirrorChange, PrRow, Result, SyncChange,
    TrackerStatusChange,
};
use ct_core::{now_ms, TrackerKind, TrackerReadiness};
use rusqlite::OptionalExtension;

impl Ledger {
    /// Este ambiente envia o espelho do projeto: é o de sync e o projeto é
    /// Jira (o hub sincroniza os GitHub sozinho).
    pub(crate) fn mirrors_tracker(&self, project_id: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT sync_role = 1 AND tracker_kind = ?2 FROM project WHERE id = ?1",
                rusqlite::params![project_id, TrackerKind::Jira.as_str()],
                |r| r.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    pub(crate) fn enqueue_issue_row(&self, owner: &str, repo: &str, i: &IssueRow) -> Result<()> {
        self.enqueue(&SyncChange::IssueMirror(IssueMirrorChange {
            owner: owner.to_string(),
            repo: repo.to_string(),
            issue: i.clone(),
            removed: false,
        }))
    }

    /// Tira do espelho as issues que saíram do JQL do projeto (o sync do
    /// Jira decide quais; a issue com work item fica sempre, por garantia).
    /// No ambiente de sync, cada uma vai ao outbox como `issue_mirror` com
    /// `removed`, para o hub apagar também. Devolve quantas saíram.
    pub fn remove_tracker_issues(&self, project_id: i64, numbers: &[i64]) -> Result<usize> {
        self.atomic(|| {
            let mirror = self.mirrors_tracker(project_id)?;
            let (owner, repo) = self.project_slug(project_id)?;
            let mut n = 0;
            for &gh in numbers {
                let Some(issue) = self.get_issue(project_id, gh)? else {
                    continue;
                };
                if self.find_work_item(project_id, gh)?.is_some() {
                    continue;
                }
                self.conn.execute(
                    "DELETE FROM issue_cache WHERE project_id = ?1 AND gh_number = ?2",
                    rusqlite::params![project_id, gh],
                )?;
                if mirror {
                    self.enqueue(&SyncChange::IssueMirror(IssueMirrorChange {
                        owner: owner.clone(),
                        repo: repo.clone(),
                        issue,
                        removed: true,
                    }))?;
                }
                n += 1;
            }
            Ok(n)
        })
    }

    /// MR do espelho sem o `work_item_id` local: o hub acha o work item pelo
    /// número dele (`gh_number`).
    pub(crate) fn enqueue_pr_row(&self, owner: &str, repo: &str, pr: &PrRow) -> Result<()> {
        let wi = self.get_work_item(pr.work_item_id)?;
        self.enqueue(&SyncChange::PrMirror(PrMirrorChange {
            owner: owner.to_string(),
            repo: repo.to_string(),
            gh_number: wi.gh_number,
            pr: MirrorPr {
                number: pr.number,
                head: pr.head.clone(),
                base: pr.base.clone(),
                state: pr.state,
                url: pr.url.clone(),
                updated_at: pr.updated_at,
                head_sha: pr.head_sha.clone(),
            },
        }))
    }

    /// Põe no outbox o espelho completo do projeto (todas as issues e todos
    /// os MRs do cache), para o hub não depender do que o ambiente de sync
    /// anterior mandou. Só no ambiente de sync; devolve quantas linhas
    /// entraram.
    pub fn enqueue_issue_mirror(&self, project_id: i64) -> Result<usize> {
        self.atomic(|| {
            if !self.mirrors_tracker(project_id)? {
                return Ok(0);
            }
            let (owner, repo) = self.project_slug(project_id)?;
            let issues = self.list_issues(project_id)?;
            for i in &issues {
                self.enqueue_issue_row(&owner, &repo, i)?;
            }
            let prs = self.gh_list_prs(project_id)?;
            for pr in &prs {
                self.enqueue_pr_row(&owner, &repo, pr)?;
            }
            Ok(issues.len() + prs.len())
        })
    }

    /// Fim de um ciclo de sync do projeto, com ou sem erro: o `sync_state`
    /// local e a prontidão conferida viram o `tracker_status` do outbox. Só
    /// no ambiente de sync; `false` = nada entrou.
    pub fn enqueue_tracker_status(
        &self,
        project_id: i64,
        readiness: &TrackerReadiness,
    ) -> Result<bool> {
        if !self.mirrors_tracker(project_id)? {
            return Ok(false);
        }
        let (owner, repo) = self.project_slug(project_id)?;
        let st = self.sync_state(project_id)?;
        self.enqueue(&SyncChange::TrackerStatus(TrackerStatusChange {
            owner,
            repo,
            last_attempt_at: st.as_ref().map_or_else(now_ms, |s| s.last_attempt_at),
            last_ok_at: st.as_ref().and_then(|s| s.last_ok_at),
            last_error: st.and_then(|s| s.last_error),
            readiness: readiness.clone(),
        }))?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use crate::test_support::ledger_with_project;
    use crate::{GithubSnapshot, IssueRow, Ledger, PrRow, SyncChange};
    use ct_core::{PrState, TrackerReadiness, WorkItemKind};

    fn issue(n: i64, title: &str) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: title.into(),
            state: "open".into(),
            labels: vec![],
            parent_number: None,
            has_sub_issues: false,
            url: format!("https://jira.exemplo.test/browse/P-{n}"),
            updated_at: 1,
            ..Default::default()
        }
    }

    fn snap(project_id: i64, issues: Vec<IssueRow>) -> GithubSnapshot {
        GithubSnapshot {
            project_id,
            issues,
            prs: vec![],
            etags: vec![],
            synced_at: 100,
        }
    }

    fn set_role(l: &Ledger, id: i64, jira: bool, sync: bool) {
        l.conn
            .execute(
                "UPDATE project SET tracker_kind = ?2, tracker_config = ?3, sync_role = ?4
                 WHERE id = ?1",
                rusqlite::params![
                    id,
                    if jira { "jira" } else { "github" },
                    if jira { r#"{"project_key":"P"}"# } else { "{}" },
                    sync
                ],
            )
            .unwrap();
    }

    fn mirrors(l: &Ledger) -> Vec<IssueRow> {
        l.outbox_pending(1000)
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.change {
                SyncChange::IssueMirror(m) => Some(m.issue),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn so_o_ambiente_de_sync_de_projeto_jira_enfileira() {
        for (jira, sync, expected) in [(true, true, 1), (true, false, 0), (false, true, 0)] {
            let (l, p) = ledger_with_project();
            set_role(&l, p.id, jira, sync);
            l.outbox_ack(i64::MAX).unwrap();
            let a = l.apply_github(&snap(p.id, vec![issue(5, "a")])).unwrap();
            assert_eq!(a.mirrored, expected, "jira={jira} sync={sync}");
            assert_eq!(mirrors(&l).len(), expected);
            let r = TrackerReadiness::default();
            assert_eq!(l.enqueue_tracker_status(p.id, &r).unwrap(), expected == 1);
            assert_eq!(l.enqueue_issue_mirror(p.id).unwrap(), expected);
        }
    }

    #[test]
    fn so_as_linhas_que_mudaram_entram() {
        let (l, p) = ledger_with_project();
        set_role(&l, p.id, true, true);
        l.apply_github(&snap(p.id, vec![issue(5, "a"), issue(6, "b")]))
            .unwrap();
        l.outbox_ack(i64::MAX).unwrap();
        let a = l
            .apply_github(&snap(p.id, vec![issue(5, "a"), issue(6, "b2")]))
            .unwrap();
        assert_eq!((a.issues, a.mirrored), (2, 1));
        let m = mirrors(&l);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].title, "b2");
        // O espelho completo leva tudo o que está no cache.
        assert_eq!(l.enqueue_issue_mirror(p.id).unwrap(), 2);
        assert_eq!(mirrors(&l).len(), 2);
    }

    fn mr(n: i64, wi: i64, state: PrState) -> PrRow {
        PrRow {
            number: n,
            work_item_id: wi,
            head: "P-5".into(),
            base: "develop".into(),
            state,
            url: format!("https://git.exemplo.test/g/r/-/merge_requests/{n}"),
            updated_at: 1,
            head_sha: Some("abc".into()),
        }
    }

    fn pr_mirrors(l: &Ledger) -> Vec<crate::PrMirrorChange> {
        l.outbox_pending(1000)
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.change {
                SyncChange::PrMirror(m) => Some(m),
                _ => None,
            })
            .collect()
    }

    /// Critério de aceite da #350: o ambiente de sync enfileira o MR que
    /// mudou como `pr_mirror`, com o número do work item; os outros não.
    #[test]
    fn mr_que_mudou_entra_como_pr_mirror_so_no_ambiente_de_sync() {
        for (sync, expected) in [(true, 1), (false, 0)] {
            let (l, p) = ledger_with_project();
            set_role(&l, p.id, true, sync);
            let wi = l.ensure_work_item(p.id, 5, WorkItemKind::Issue).unwrap();
            l.outbox_ack(i64::MAX).unwrap();
            let mut s = snap(p.id, vec![]);
            s.prs = vec![mr(12, wi.id, PrState::Open)];
            let a = l.apply_github(&s).unwrap();
            assert_eq!(a.mirrored_prs, expected, "sync={sync}");
            let m = pr_mirrors(&l);
            assert_eq!(m.len(), expected);
            if sync {
                assert_eq!((m[0].gh_number, m[0].pr.number), (5, 12));
                assert_eq!(m[0].pr.state, PrState::Open);
                assert_eq!(m[0].pr.head_sha.as_deref(), Some("abc"));
                assert!(m[0].pr.url.ends_with("/-/merge_requests/12"));

                // Igual: nada novo; mesclado: entra de novo.
                l.outbox_ack(i64::MAX).unwrap();
                assert_eq!(l.apply_github(&s).unwrap().mirrored_prs, 0);
                s.prs = vec![mr(12, wi.id, PrState::Merged)];
                assert_eq!(l.apply_github(&s).unwrap().mirrored_prs, 1);
                assert_eq!(pr_mirrors(&l)[0].pr.state, PrState::Merged);
                // O espelho completo leva os MRs junto das issues.
                l.outbox_ack(i64::MAX).unwrap();
                assert_eq!(l.enqueue_issue_mirror(p.id).unwrap(), 1);
                assert_eq!(pr_mirrors(&l).len(), 1);
            }
        }
    }

    #[test]
    fn status_leva_o_sync_state_e_a_prontidao() {
        let (l, p) = ledger_with_project();
        set_role(&l, p.id, true, true);
        l.apply_github(&snap(p.id, vec![])).unwrap();
        l.record_sync_error(p.id, "o Jira recusou a senha (401)")
            .unwrap();
        l.outbox_ack(i64::MAX).unwrap();
        let r = TrackerReadiness {
            owner: "o".into(),
            checked_at: 7,
            ..TrackerReadiness::default()
        };
        assert!(l.enqueue_tracker_status(p.id, &r).unwrap());
        let out = l.outbox_pending(10).unwrap();
        let SyncChange::TrackerStatus(t) = &out[0].change else {
            panic!("{out:?}");
        };
        assert_eq!(t.last_ok_at, Some(100));
        assert_eq!(
            t.last_error.as_deref(),
            Some("o Jira recusou a senha (401)")
        );
        assert!(t.last_attempt_at >= 100);
        assert_eq!(t.readiness, r);
    }
}
