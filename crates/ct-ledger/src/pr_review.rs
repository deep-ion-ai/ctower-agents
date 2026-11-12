//! Feedback de review dos PRs abertos dos work items no espelho (#130,
//! épico #136): as threads de review não resolvidas e os reviews que pedem
//! mudanças, lidos pelo sync (`ct_gh::reviews`) e gravados em
//! `pr_review_item`, no `ledger.db` e no `hub.db`. A coluna
//! `pr_cache.reviews_updated_at` guarda o `updated_at` do PR na última
//! leitura: o sync só relê o PR aberto cujo `updated_at` mudou desde então
//! (ou que nunca foi lido).
//!
//! Quem lê (a ferramenta `pr_feedback` do agente, #133, e o pedido ao
//! agente autônomo, #184) usa [`PrFeedback`]: cada comentário traz o autor,
//! a associação dele com o repositório e a marca [`ReviewThread::external`]
//! de quem não é `OWNER`, `MEMBER` nem `COLLABORATOR`. O texto do
//! comentário é dado de terceiros: quem o repassa ao agente põe entre
//! delimitadores, nunca como instrução.

use crate::github::{GhMirror, PR_COLS};
use crate::{Hub, Ledger, PrRow, Result};
use ct_core::ForgeKind;
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

/// Associações do autor que contam como da casa; as outras (`CONTRIBUTOR`,
/// `FIRST_TIME_CONTRIBUTOR`, `FIRST_TIMER`, `MANNEQUIN`, `NONE`, vazia)
/// marcam o comentário como externo.
pub const TRUSTED_ASSOCIATIONS: &[&str] = &["OWNER", "MEMBER", "COLLABORATOR"];

/// O comentário de quem tem esta `authorAssociation` é externo?
pub fn is_external_association(association: &str) -> bool {
    !TRUSTED_ASSOCIATIONS.contains(&association)
}

/// Thread de review não resolvida, pelo primeiro comentário dela.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewThread {
    /// Id da thread no GitHub (GraphQL `node_id`).
    pub id: String,
    /// Arquivo comentado.
    pub path: String,
    /// Linha no arquivo (a atual ou, se o trecho mudou, a original);
    /// `None` = comentário no arquivo inteiro.
    pub line: Option<i64>,
    /// O diff comentado mudou desde o comentário.
    pub outdated: bool,
    /// Login do autor do primeiro comentário (`ghost` = conta apagada).
    pub author: String,
    /// `authorAssociation` do autor (`OWNER`, `MEMBER`, `COLLABORATOR`,
    /// `CONTRIBUTOR`, `NONE`...).
    pub author_association: String,
    /// O autor não é `OWNER`, `MEMBER` nem `COLLABORATOR`.
    pub external: bool,
    pub body: String,
    pub url: String,
    /// Respostas na thread, além do primeiro comentário.
    pub replies: i64,
    /// Criação do primeiro comentário (ms Unix).
    pub created_at: i64,
}

/// Review vigente de um revisor que pede mudanças (`CHANGES_REQUESTED`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangesRequest {
    /// Id do review no GitHub (GraphQL `node_id`).
    pub id: String,
    pub author: String,
    pub author_association: String,
    /// O autor não é `OWNER`, `MEMBER` nem `COLLABORATOR`.
    pub external: bool,
    /// Corpo do review (pode ser vazio: os comentários vêm nas threads).
    pub body: String,
    pub url: String,
    /// Envio do review (ms Unix).
    pub submitted_at: i64,
}

/// Leitura das reviews de um PR pelo sync, aplicada com
/// [`Ledger::apply_pr_reviews`] / [`Hub::apply_pr_reviews`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PrReviews {
    pub number: i64,
    /// `updated_at` do PR em que a leitura se baseou (o de
    /// [`ReviewTarget::updated_at`]).
    pub pr_updated_at: i64,
    /// Só as não resolvidas.
    pub threads: Vec<ReviewThread>,
    pub changes_requested: Vec<ChangesRequest>,
}

/// PR aberto de work item cujas reviews o sync precisa (re)ler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewTarget {
    pub number: i64,
    /// `updated_at` do PR no espelho agora.
    pub updated_at: i64,
}

/// PR do espelho com o feedback de review gravado.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrFeedback {
    pub pr: PrRow,
    /// `updated_at` do PR na última leitura das reviews; `None` = ainda não
    /// lidas (PR que o sync ainda não visitou, ou não aberto).
    pub read_at_updated: Option<i64>,
    /// Threads não resolvidas, da mais antiga para a mais nova.
    pub threads: Vec<ReviewThread>,
    /// Reviews vigentes que pedem mudanças, do mais antigo ao mais novo.
    pub changes_requested: Vec<ChangesRequest>,
}

impl PrFeedback {
    /// Há algo de autor externo (que não entra no pedido ao agente por
    /// padrão, épico #136).
    pub fn has_external(&self) -> bool {
        self.threads.iter().any(|t| t.external) || self.changes_requested.iter().any(|c| c.external)
    }
}

const KIND_THREAD: &str = "thread";
const KIND_CHANGES: &str = "changes_requested";

const ITEM_COLS: &str =
    "node_id, path, line, outdated, author, author_association, body, url, replies, created_at";

fn row_to_thread(r: &Row) -> rusqlite::Result<ReviewThread> {
    let association: String = r.get(5)?;
    Ok(ReviewThread {
        id: r.get(0)?,
        path: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        line: r.get(2)?,
        outdated: r.get(3)?,
        author: r.get(4)?,
        external: is_external_association(&association),
        author_association: association,
        body: r.get(6)?,
        url: r.get(7)?,
        replies: r.get(8)?,
        created_at: r.get(9)?,
    })
}

fn row_to_changes(r: &Row) -> rusqlite::Result<ChangesRequest> {
    let association: String = r.get(5)?;
    Ok(ChangesRequest {
        id: r.get(0)?,
        author: r.get(4)?,
        external: is_external_association(&association),
        author_association: association,
        body: r.get(6)?,
        url: r.get(7)?,
        submitted_at: r.get(9)?,
    })
}

impl GhMirror<'_> {
    /// PRs abertos de work items do projeto (forja GitHub) com as reviews
    /// por ler: nunca lidas ou lidas com outro `updated_at`.
    pub(crate) fn pr_review_targets(&self, project_id: i64) -> Result<Vec<ReviewTarget>> {
        let mut stmt = self.0.prepare(
            "SELECT c.number, c.updated_at FROM pr_cache c
             JOIN project p ON p.id = c.project_id
             JOIN work_item w ON w.id = c.work_item_id
             WHERE c.project_id = ?1 AND c.state = 'open' AND p.forge_kind = ?2
               AND (c.reviews_updated_at IS NULL OR c.reviews_updated_at <> c.updated_at)
             ORDER BY c.number",
        )?;
        let rows = stmt.query_map(params![project_id, ForgeKind::Github.as_str()], |r| {
            Ok(ReviewTarget {
                number: r.get(0)?,
                updated_at: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Troca o feedback gravado de cada PR pelo lido. PR que saiu do
    /// espelho nesse meio-tempo fica de fora. Devolve quantos PRs gravou.
    /// Quem chama abre a transação.
    pub(crate) fn apply_pr_reviews(&self, project_id: i64, reviews: &[PrReviews]) -> Result<usize> {
        let mut n = 0;
        for r in reviews {
            let updated = self.0.execute(
                "UPDATE pr_cache SET reviews_updated_at = ?3 WHERE project_id = ?1 AND number = ?2",
                params![project_id, r.number, r.pr_updated_at],
            )?;
            if updated == 0 {
                continue;
            }
            self.0.execute(
                "DELETE FROM pr_review_item WHERE project_id = ?1 AND pr_number = ?2",
                params![project_id, r.number],
            )?;
            let mut insert = self.0.prepare_cached(
                "INSERT OR REPLACE INTO pr_review_item
                   (project_id, pr_number, kind, node_id, path, line, outdated, author,
                    author_association, body, url, replies, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            )?;
            for t in &r.threads {
                insert.execute(params![
                    project_id,
                    r.number,
                    KIND_THREAD,
                    t.id,
                    t.path,
                    t.line,
                    t.outdated,
                    t.author,
                    t.author_association,
                    t.body,
                    t.url,
                    t.replies,
                    t.created_at
                ])?;
            }
            for c in &r.changes_requested {
                insert.execute(params![
                    project_id,
                    r.number,
                    KIND_CHANGES,
                    c.id,
                    Option::<String>::None,
                    Option::<i64>::None,
                    false,
                    c.author,
                    c.author_association,
                    c.body,
                    c.url,
                    0,
                    c.submitted_at
                ])?;
            }
            n += 1;
        }
        Ok(n)
    }

    fn review_items<T>(
        &self,
        project_id: i64,
        pr_number: i64,
        kind: &str,
        map: fn(&Row) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>> {
        let mut stmt = self.0.prepare_cached(&format!(
            "SELECT {ITEM_COLS} FROM pr_review_item
             WHERE project_id = ?1 AND pr_number = ?2 AND kind = ?3
             ORDER BY created_at, node_id"
        ))?;
        let rows = stmt.query_map(params![project_id, pr_number, kind], map)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn feedback_of(&self, project_id: i64, pr: PrRow) -> Result<PrFeedback> {
        let read_at_updated: Option<i64> = self.0.query_row(
            "SELECT reviews_updated_at FROM pr_cache WHERE project_id = ?1 AND number = ?2",
            params![project_id, pr.number],
            |r| r.get(0),
        )?;
        Ok(PrFeedback {
            threads: self.review_items(project_id, pr.number, KIND_THREAD, row_to_thread)?,
            changes_requested: self.review_items(
                project_id,
                pr.number,
                KIND_CHANGES,
                row_to_changes,
            )?,
            read_at_updated,
            pr,
        })
    }

    pub(crate) fn pr_feedback(&self, project_id: i64, number: i64) -> Result<Option<PrFeedback>> {
        let Some(pr) = self.get_pr(project_id, number)? else {
            return Ok(None);
        };
        self.feedback_of(project_id, pr).map(Some)
    }

    pub(crate) fn pr_feedback_for_work_item(&self, work_item_id: i64) -> Result<Vec<PrFeedback>> {
        let project_id: Option<i64> = self
            .0
            .query_row(
                "SELECT project_id FROM work_item WHERE id = ?1",
                [work_item_id],
                |r| r.get(0),
            )
            .optional()?;
        let Some(project_id) = project_id else {
            return Ok(Vec::new());
        };
        let mut stmt = self.0.prepare(&format!(
            "SELECT {PR_COLS} FROM pr_cache WHERE work_item_id = ?1 ORDER BY number DESC"
        ))?;
        let prs = stmt
            .query_map([work_item_id], crate::github::row_to_pr)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        prs.into_iter()
            .map(|pr| self.feedback_of(project_id, pr))
            .collect()
    }
}

impl Ledger {
    /// PRs abertos de work items do projeto cujas reviews o sync deve
    /// (re)ler: nunca lidas, ou lidas com outro `updated_at` do PR.
    pub fn pr_review_targets(&self, project_id: i64) -> Result<Vec<ReviewTarget>> {
        self.gh().pr_review_targets(project_id)
    }

    /// Grava as reviews lidas, numa transação. Devolve quantos PRs gravou.
    pub fn apply_pr_reviews(&self, project_id: i64, reviews: &[PrReviews]) -> Result<usize> {
        self.atomic(|| self.gh().apply_pr_reviews(project_id, reviews))
    }

    /// Feedback de review gravado de um PR; `None` = PR fora do espelho.
    pub fn pr_feedback(&self, project_id: i64, number: i64) -> Result<Option<PrFeedback>> {
        self.gh().pr_feedback(project_id, number)
    }

    /// Feedback de review de cada PR do work item, do mais novo ao mais
    /// antigo (como [`Ledger::prs_for_work_item`]). Os PRs não abertos
    /// trazem o que foi lido enquanto estavam abertos; filtre por
    /// `pr.state` se só os abertos interessam.
    pub fn pr_feedback_for_work_item(&self, work_item_id: i64) -> Result<Vec<PrFeedback>> {
        self.gh().pr_feedback_for_work_item(work_item_id)
    }
}

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
    use crate::{GithubSnapshot, IssueRow, NewHubProject, NewProject, Project};
    use ct_core::{PrState, WorkItemKind};

    fn ledger() -> (Ledger, Project, i64) {
        let l = Ledger::open_in_memory().unwrap();
        let p = l
            .add_project(&NewProject {
                name: "acme-web".into(),
                gh_owner: "acme".into(),
                gh_repo: "acme-web".into(),
                repo_path: "/srv/acme-web".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        let wi = l.ensure_work_item(p.id, 7, WorkItemKind::Issue).unwrap();
        (l, p, wi.id)
    }

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

    fn changes(id: &str, assoc: &str) -> ChangesRequest {
        ChangesRequest {
            id: id.into(),
            author: "beltrano".into(),
            author_association: assoc.into(),
            external: is_external_association(assoc),
            body: "faltou teste".into(),
            url: "https://github.com/acme/acme-web/pull/30#pullrequestreview-1".into(),
            submitted_at: 5,
        }
    }

    #[test]
    fn associacao_externa() {
        for a in ["OWNER", "MEMBER", "COLLABORATOR"] {
            assert!(!is_external_association(a), "{a}");
        }
        for a in [
            "CONTRIBUTOR",
            "FIRST_TIME_CONTRIBUTOR",
            "FIRST_TIMER",
            "NONE",
            "",
            "member",
        ] {
            assert!(is_external_association(a), "{a}");
        }
    }

    /// Alvos: só o PR aberto de work item, nunca lido ou com outro
    /// `updated_at`; gravar troca o feedback inteiro do PR.
    #[test]
    fn alvos_gravacao_e_leitura() {
        let (l, p, wi) = ledger();
        l.apply_github(&snap(
            p.id,
            vec![
                pr(30, wi, PrState::Open, 10),
                pr(31, wi, PrState::Merged, 10),
            ],
        ))
        .unwrap();
        let alvo = ReviewTarget {
            number: 30,
            updated_at: 10,
        };
        assert_eq!(l.pr_review_targets(p.id).unwrap(), [alvo]);
        let f = l.pr_feedback(p.id, 30).unwrap().unwrap();
        assert_eq!(f.read_at_updated, None);
        assert!(f.threads.is_empty());

        let lido = PrReviews {
            number: 30,
            pr_updated_at: 10,
            threads: vec![
                thread("T2", "src/b.rs", None, "NONE", 2),
                thread("T1", "src/a.rs", Some(12), "MEMBER", 1),
            ],
            changes_requested: vec![changes("R1", "OWNER")],
        };
        assert_eq!(l.apply_pr_reviews(p.id, &[lido]).unwrap(), 1);
        assert!(l.pr_review_targets(p.id).unwrap().is_empty());
        let f = l.pr_feedback(p.id, 30).unwrap().unwrap();
        assert_eq!(f.read_at_updated, Some(10));
        let t: Vec<(&str, &str, Option<i64>, bool)> = f
            .threads
            .iter()
            .map(|t| (t.id.as_str(), t.path.as_str(), t.line, t.external))
            .collect();
        assert_eq!(
            t,
            [
                ("T1", "src/a.rs", Some(12), false),
                ("T2", "src/b.rs", None, true)
            ]
        );
        assert_eq!(f.changes_requested, [changes("R1", "OWNER")]);
        assert!(f.has_external());

        // Por work item: os dois PRs, do mais novo ao mais antigo.
        let por_item = l.pr_feedback_for_work_item(wi).unwrap();
        assert_eq!(por_item.len(), 2);
        assert_eq!(por_item[0].pr.number, 31);
        assert_eq!(por_item[1].threads.len(), 2);

        // O PR mudou: volta a ser alvo; a nova leitura troca tudo.
        l.apply_github(&snap(p.id, vec![pr(30, wi, PrState::Open, 20)]))
            .unwrap();
        assert_eq!(
            l.pr_review_targets(p.id).unwrap(),
            [ReviewTarget {
                number: 30,
                updated_at: 20
            }]
        );
        let relido = PrReviews {
            number: 30,
            pr_updated_at: 20,
            threads: vec![],
            changes_requested: vec![changes("R2", "CONTRIBUTOR")],
        };
        l.apply_pr_reviews(p.id, &[relido]).unwrap();
        let f = l.pr_feedback(p.id, 30).unwrap().unwrap();
        assert!(f.threads.is_empty());
        assert_eq!(f.changes_requested.len(), 1);
        assert!(f.changes_requested[0].external);

        // PR fora do espelho: não grava nada.
        let fora = PrReviews {
            number: 99,
            pr_updated_at: 1,
            ..PrReviews::default()
        };
        assert_eq!(l.apply_pr_reviews(p.id, &[fora]).unwrap(), 0);
        assert!(l.pr_feedback(p.id, 99).unwrap().is_none());
    }

    /// O feedback sai junto com o PR do `pr_cache` (gatilho da migração).
    #[test]
    fn feedback_sai_com_o_pr() {
        let (l, p, wi) = ledger();
        l.apply_github(&snap(p.id, vec![pr(30, wi, PrState::Open, 10)]))
            .unwrap();
        l.apply_pr_reviews(
            p.id,
            &[PrReviews {
                number: 30,
                pr_updated_at: 10,
                threads: vec![thread("T1", "a.rs", Some(1), "MEMBER", 1)],
                changes_requested: vec![],
            }],
        )
        .unwrap();
        let count = |l: &Ledger| -> i64 {
            l.conn
                .query_row("SELECT count(*) FROM pr_review_item", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(count(&l), 1);
        l.conn
            .execute("DELETE FROM pr_cache WHERE number = 30", [])
            .unwrap();
        assert_eq!(count(&l), 0);
    }

    /// O mesmo no `hub.db`.
    #[test]
    fn hub_grava_e_le() {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project(&NewHubProject {
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
