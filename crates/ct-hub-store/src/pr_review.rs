//! Reviews e feedback de PR no `hub.db` (espelho do que o `ledger.db` guarda
//! em [`ct_ledger::Ledger`]). Os tipos ficam no `ct-ledger` (`pr_review`).

use crate::github::GhMirror;
use crate::Hub;
use ct_ledger::{PrFeedback, PrReviews, Result, ReviewTarget};

impl Hub {
    /// [`Ledger::pr_review_targets`] no `hub.db`.
    pub fn pr_review_targets(&self, project_id: i64) -> Result<Vec<ReviewTarget>> {
        GhMirror(self.conn()).pr_review_targets(project_id)
    }

    /// [`Ledger::apply_pr_reviews`] no `hub.db`.
    pub fn apply_pr_reviews(&self, project_id: i64, reviews: &[PrReviews]) -> Result<usize> {
        let tx = self.tx()?;
        let n = GhMirror(self.conn()).apply_pr_reviews(project_id, reviews)?;
        tx.commit()?;
        Ok(n)
    }

    /// [`Ledger::pr_feedback`] no `hub.db`.
    pub fn pr_feedback(&self, project_id: i64, number: i64) -> Result<Option<PrFeedback>> {
        GhMirror(self.conn()).pr_feedback(project_id, number)
    }

    /// [`Ledger::pr_feedback_for_work_item`] no `hub.db`.
    pub fn pr_feedback_for_work_item(&self, work_item_id: i64) -> Result<Vec<PrFeedback>> {
        GhMirror(self.conn()).pr_feedback_for_work_item(work_item_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_core::PrState;
    use ct_ledger::{is_external_association, GithubSnapshot, IssueRow, PrRow, ReviewThread};

    fn pr(n: i64, wi: i64, state: PrState, updated_at: i64) -> PrRow {
        PrRow {
            number: n,
            work_item_id: wi,
            head: "feat/7-x".into(),
            base: "develop".into(),
            state,
            url: format!("https://github.com/acme/acme-web/pull/{n}"),
            updated_at,
            head_sha: None,
        }
    }

    fn snap(project_id: i64, prs: Vec<PrRow>) -> GithubSnapshot {
        GithubSnapshot {
            project_id,
            prs,
            synced_at: 100,
            ..Default::default()
        }
    }

    fn thread(id: &str, path: &str, line: Option<i64>, assoc: &str, at: i64) -> ReviewThread {
        ReviewThread {
            id: id.into(),
            path: path.into(),
            line,
            outdated: false,
            author: "fulano".into(),
            author_association: assoc.into(),
            external: is_external_association(assoc),
            body: format!("ajuste em {path}"),
            url: format!("https://github.com/acme/acme-web/pull/30#discussion_{id}"),
            replies: 1,
            created_at: at,
        }
    }

    /// O mesmo no `hub.db`.
    #[test]
    fn hub_grava_e_le() {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project(&crate::NewHubProject {
            owner: "acme".into(),
            repo: "acme-web".into(),
            name: "acme-web".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        let pid = hub.find_project("acme", "acme-web").unwrap().unwrap().id;
        let issue = IssueRow {
            gh_number: 7,
            title: "Issue 7".into(),
            state: "open".into(),
            url: "https://github.com/acme/acme-web/issues/7".into(),
            updated_at: 1,
            ..Default::default()
        };
        let mut s = snap(pid, vec![]);
        s.issues = vec![issue];
        hub.apply_github(&s, "hub").unwrap();
        let wi = hub
            .set_status_manual(
                "acme",
                "acme-web",
                7,
                ct_core::WorkItemStatus::EmExecucao,
                None,
                "hub",
            )
            .unwrap();
        hub.apply_github(&snap(pid, vec![pr(30, wi.id, PrState::Open, 10)]), "hub")
            .unwrap();
        assert_eq!(hub.pr_review_targets(pid).unwrap().len(), 1);
        hub.apply_pr_reviews(
            pid,
            &[PrReviews {
                number: 30,
                pr_updated_at: 10,
                threads: vec![thread("T1", "a.rs", Some(1), "NONE", 1)],
                changes_requested: vec![],
            }],
        )
        .unwrap();
        assert!(hub.pr_review_targets(pid).unwrap().is_empty());
        let f = hub.pr_feedback_for_work_item(wi.id).unwrap();
        assert!(f[0].threads[0].external);
        assert_eq!(f[0].threads[0].line, Some(1));
    }
}
