//! Prioridade e ordem dos work items e dos projetos (issue #215, spec do
//! gestor §4). Tudo aqui é só do hub: a ingestão nunca escreve essas colunas,
//! e cada mudança grava um evento com `env_id = hub_id`, como o status
//! manual.
//!
//! Uma **classe** é (projeto, prioridade). Dentro dela, `rank` é denso
//! (1, 2, 3...) entre os itens que estão no ranking: não concluídos e com a
//! issue aberta no espelho. Os demais ficam com `rank` nulo. Itens não
//! triados (prioridade nula) não têm `rank`.

use super::{row_to_project, Hub, HubProject, HubWorkItem, PROJECT_COLS};
use crate::sync::project_key;
use crate::{LedgerError, Result};
use ct_core::{now_ms, Priority, WorkItemKind, WorkItemStatus};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// Item de `hub.ranking.get`, na ordem da §4.3 da spec do gestor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubRankingItem {
    /// 1, 2, 3... dentro da resposta (depois dos filtros).
    pub position: u32,
    pub owner: String,
    pub repo: String,
    pub number: i64,
    /// Referência pronta (`ct_core::item_ref`): `owner/repo#n` ou `PROJ-n`.
    #[serde(rename = "ref", default)]
    #[cfg_attr(feature = "ts", ts(rename = "ref"))]
    pub item_ref: String,
    /// Nulo quando a issue ainda não está no espelho.
    pub title: Option<String>,
    pub kind: WorkItemKind,
    /// `backlog` para issue sem work item.
    pub status: WorkItemStatus,
    pub priority: Option<Priority>,
    pub rank: Option<i64>,
}

/// Resposta de `hub.ranking.get`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubRanking {
    pub items: Vec<HubRankingItem>,
    /// A lista foi cortada em `limit`.
    pub truncated: bool,
}

/// Filtros de `Hub::ranking`.
#[derive(Debug, Clone, Default)]
pub struct RankingFilter {
    /// `owner/repo`; `None` = todos os projetos.
    pub project: Option<(String, String)>,
    pub priority: Option<Priority>,
}

/// Padrão e teto do `limit` das listas para agentes (spec do gestor §6).
pub const RANKING_LIMIT: u32 = 50;
pub const RANKING_MAX: u32 = 200;

/// Onde pôr o item ao mover: antes ou depois de outro da mesma classe, ou
/// no fim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placement<T> {
    Before(T),
    After(T),
    End,
}

impl<T: PartialEq + Copy> Placement<T> {
    /// Insere `item` em `order` (que não o contém). `false` se a referência
    /// não está em `order`.
    fn apply(self, order: &mut Vec<i64>, item: i64, find: impl Fn(T) -> Option<i64>) -> bool {
        let at = match self {
            Placement::End => order.len(),
            Placement::Before(r) | Placement::After(r) => {
                let Some(id) = find(r) else { return false };
                let Some(i) = order.iter().position(|&o| o == id) else {
                    return false;
                };
                if matches!(self, Placement::After(_)) {
                    i + 1
                } else {
                    i
                }
            }
        };
        order.insert(at, item);
        true
    }
}

/// `cause` (e `proposal_id`) dos eventos de prioridade e ordem.
pub(super) fn cause(payload: &mut serde_json::Value, proposal_id: Option<i64>) {
    match proposal_id {
        Some(id) => {
            payload["cause"] = json!("proposal");
            payload["proposal_id"] = json!(id);
        }
        None => payload["cause"] = json!("manual"),
    }
}

impl Hub {
    /// Ids da classe que estão no ranking, na ordem atual.
    fn class_order(&self, project_id: i64, priority: Priority) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT w.id FROM work_item w
             LEFT JOIN issue_cache i ON i.project_id = w.project_id AND i.gh_number = w.gh_number
             WHERE w.project_id = ?1 AND w.priority = ?2 AND w.status <> 'concluido'
               AND coalesce(i.state, 'open') = 'open'
             ORDER BY w.rank NULLS LAST, w.gh_number",
        )?;
        let rows = stmt.query_map(params![project_id, priority.as_str()], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Reescreve os `rank` da classe: `order` recebe 1, 2, 3...; o resto da
    /// classe (fora do ranking) fica nulo.
    fn write_class(&self, project_id: i64, priority: Priority, order: &[i64]) -> Result<()> {
        self.conn.execute(
            "UPDATE work_item SET rank = NULL WHERE project_id = ?1 AND priority = ?2",
            params![project_id, priority.as_str()],
        )?;
        let mut stmt = self
            .conn
            .prepare("UPDATE work_item SET rank = ?2 WHERE id = ?1")?;
        for (i, id) in order.iter().enumerate() {
            stmt.execute(params![id, i as i64 + 1])?;
        }
        Ok(())
    }

    fn in_ranking(&self, wi: &HubWorkItem) -> Result<bool> {
        if wi.status == WorkItemStatus::Concluido {
            return Ok(false);
        }
        Ok(self
            .get_issue(wi.project_id, wi.gh_number)?
            .is_none_or(|i| i.state == "open"))
    }

    /// `hub.workitem.set_priority`. Leva o item ao fim da nova classe e
    /// fecha o buraco na antiga. Issue sem work item ganha um, como no status
    /// manual. Mesma prioridade = nada a gravar. `proposal_id` = mudança
    /// aplicada por uma proposta aprovada (#218).
    #[allow(clippy::too_many_arguments)]
    pub fn set_priority(
        &self,
        owner: &str,
        repo: &str,
        gh_number: i64,
        priority: Option<Priority>,
        reason: Option<&str>,
        proposal_id: Option<i64>,
        env_id: &str,
    ) -> Result<HubWorkItem> {
        // `find_project`: o sentinela do gestor não tem prioridade nem
        // status manual.
        let Some(pid) = self.find_project(owner, repo)?.map(|p| p.id) else {
            return Err(LedgerError::NotFound(
                "projeto",
                format!("{owner}/{repo} não está no hub"),
            ));
        };
        let tx = self.tx()?;
        let wi = match self.find_work_item(owner, repo, gh_number)? {
            Some(w) => w,
            None => self.work_item(self.manual_work_item(pid, owner, repo, gh_number)?)?,
        };
        if wi.priority != priority {
            self.conn.execute(
                "UPDATE work_item SET priority = ?2, rank = NULL, priority_at = ?3 WHERE id = ?1",
                params![wi.id, priority.map(Priority::as_str), now_ms()],
            )?;
            if let Some(old) = wi.priority {
                let order = self.class_order(pid, old)?;
                self.write_class(pid, old, &order)?;
            }
            if let Some(new) = priority {
                let mut order = self.class_order(pid, new)?;
                order.retain(|&id| id != wi.id);
                if self.in_ranking(&wi)? {
                    order.push(wi.id);
                }
                self.write_class(pid, new, &order)?;
            }
            let rank_to = self.work_item(wi.id)?.rank;
            let mut payload = json!({
                "from": wi.priority, "to": priority, "rank_from": wi.rank, "rank_to": rank_to,
            });
            cause(&mut payload, proposal_id);
            if let Some(r) = reason.map(str::trim).filter(|r| !r.is_empty()) {
                payload["reason"] = json!(r);
            }
            self.hub_event(env_id, pid, Some(wi.id), "priority_set", &payload)?;
            self.obsolete_changed(wi.id)?;
        }
        tx.commit()?;
        self.work_item(wi.id)
    }

    /// `hub.workitem.move`: muda a posição dentro da mesma classe, antes ou
    /// depois de outro item (pelo número), ou para o fim. Reescreve a classe
    /// inteira numa transação. Mesma posição = nada a gravar.
    pub fn move_work_item(
        &self,
        owner: &str,
        repo: &str,
        gh_number: i64,
        to: Placement<i64>,
        proposal_id: Option<i64>,
        env_id: &str,
    ) -> Result<HubWorkItem> {
        let Some(wi) = self.find_work_item(owner, repo, gh_number)? else {
            return Err(LedgerError::NotFound(
                "work item",
                format!("{owner}/{repo}#{gh_number} não está no hub"),
            ));
        };
        let Some(priority) = wi.priority else {
            return Err(LedgerError::Conflict(format!(
                "{owner}/{repo}#{gh_number} não está triado: defina a prioridade antes de mover"
            )));
        };
        if !self.in_ranking(&wi)? {
            return Err(LedgerError::Conflict(format!(
                "{owner}/{repo}#{gh_number} está fora do ranking (concluído ou com a issue fechada)"
            )));
        }
        let tx = self.tx()?;
        let before = self.class_order(wi.project_id, priority)?;
        let mut order = before.clone();
        order.retain(|&id| id != wi.id);
        let placed = to.apply(&mut order, wi.id, |n| {
            self.find_work_item(owner, repo, n)
                .ok()
                .flatten()
                .map(|w| w.id)
        });
        if !placed {
            let r = match to {
                Placement::Before(n) | Placement::After(n) => n,
                Placement::End => unreachable!("fim sempre cabe"),
            };
            return Err(LedgerError::Conflict(format!(
                "{owner}/{repo}#{r} não está na classe {priority} do projeto (ou está fora do ranking)"
            )));
        }
        // `rank` pode ter buraco (um item da classe foi concluído depois da
        // última reescrita): compara a ordem e o valor gravado.
        let dense = before
            .iter()
            .enumerate()
            .all(|(i, &id)| self.work_item(id).ok().and_then(|w| w.rank) == Some(i as i64 + 1));
        if order != before || !dense {
            self.write_class(wi.project_id, priority, &order)?;
            let rank_to = self.work_item(wi.id)?.rank;
            if rank_to != wi.rank {
                self.conn.execute(
                    "UPDATE work_item SET priority_at = ?2 WHERE id = ?1",
                    params![wi.id, now_ms()],
                )?;
                let mut payload = json!({
                    "priority": priority, "rank_from": wi.rank, "rank_to": rank_to,
                });
                cause(&mut payload, proposal_id);
                self.hub_event(env_id, wi.project_id, Some(wi.id), "rank_moved", &payload)?;
            }
        }
        tx.commit()?;
        self.work_item(wi.id)
    }

    /// `hub.projects.move`: ordem entre projetos, que desempata itens da
    /// mesma prioridade no ranking do portfólio. Reescreve os `rank` de
    /// todos os projetos na ordem atual (nulo = fim, por nome).
    pub fn move_project(
        &self,
        owner: &str,
        repo: &str,
        to: Placement<(&str, &str)>,
        env_id: &str,
    ) -> Result<HubProject> {
        let Some(project) = self.find_project(owner, repo)? else {
            return Err(LedgerError::NotFound(
                "projeto",
                format!("{owner}/{repo} não está no hub"),
            ));
        };
        let tx = self.tx()?;
        let all = self.list_projects()?;
        let before: Vec<i64> = all.iter().map(|p| p.id).collect();
        let mut order = before.clone();
        order.retain(|&id| id != project.id);
        let placed = to.apply(&mut order, project.id, |(o, r)| {
            let key = project_key(o, r);
            all.iter()
                .find(|p| project_key(&p.gh_owner, &p.gh_repo) == key)
                .map(|p| p.id)
        });
        if !placed {
            let (o, r) = match to {
                Placement::Before(x) | Placement::After(x) => x,
                Placement::End => unreachable!("fim sempre cabe"),
            };
            return Err(LedgerError::NotFound(
                "projeto",
                format!("{o}/{r} não está no hub"),
            ));
        }
        let dense = all
            .iter()
            .enumerate()
            .all(|(i, p)| p.rank == Some(i as i64 + 1));
        if order != before || !dense {
            let mut stmt = self
                .conn
                .prepare("UPDATE project SET rank = ?2 WHERE id = ?1")?;
            for (i, id) in order.iter().enumerate() {
                stmt.execute(params![id, i as i64 + 1])?;
            }
            let rank_to = order.iter().position(|&id| id == project.id).unwrap() as i64 + 1;
            if project.rank != Some(rank_to) {
                let payload = json!({ "rank_from": project.rank, "rank_to": rank_to });
                self.hub_event(env_id, project.id, None, "project_rank_moved", &payload)?;
            }
        }
        tx.commit()?;
        Ok(self.conn.query_row(
            &format!("SELECT {PROJECT_COLS} FROM project WHERE id = ?1"),
            [project.id],
            row_to_project,
        )?)
    }

    /// `hub.ranking.get`: issues abertas do espelho e work items fora dele,
    /// sem os concluídos, na ordem da §4.3 da spec do gestor (a classe
    /// primeiro, o projeto só desempata). Não triados vêm no fim.
    pub fn ranking(&self, f: &RankingFilter, limit: u32) -> Result<HubRanking> {
        let limit = limit.clamp(1, RANKING_MAX);
        let key = f.project.as_ref().map(|(o, r)| project_key(o, r));
        let mut stmt = self.conn.prepare(
            "SELECT p.gh_owner, p.gh_repo, x.number, x.title, x.kind, x.status, x.priority, x.rank,
                    p.tracker_kind, p.tracker_config
             FROM (
               SELECT i.project_id AS pid, i.gh_number AS number, i.title AS title,
                      coalesce(w.kind, CASE WHEN i.has_sub_issues THEN 'epic' ELSE 'issue' END)
                        AS kind,
                      coalesce(w.status, 'backlog') AS status, w.priority AS priority,
                      w.rank AS rank
               FROM issue_cache i
               LEFT JOIN work_item w ON w.project_id = i.project_id AND w.gh_number = i.gh_number
               WHERE i.state = 'open' AND coalesce(w.status, 'backlog') <> 'concluido'
               UNION ALL
               SELECT w.project_id, w.gh_number, NULL, w.kind, w.status, w.priority, w.rank
               FROM work_item w
               WHERE w.status <> 'concluido' AND w.gh_number > 0
                 AND NOT EXISTS (SELECT 1 FROM issue_cache i
                                 WHERE i.project_id = w.project_id AND i.gh_number = w.gh_number)
             ) x
             JOIN project p ON p.id = x.pid AND p.deleted_at IS NULL AND p.kind = 'github'
             WHERE (?1 IS NULL OR p.key = ?1) AND (?2 IS NULL OR x.priority = ?2)
             ORDER BY x.priority NULLS LAST, p.rank NULLS LAST, p.name, p.key,
                      x.rank NULLS LAST, x.number
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![key, f.priority.map(Priority::as_str), limit + 1],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    crate::parse_col::<WorkItemKind>(r, 4)?,
                    crate::parse_col::<WorkItemStatus>(r, 5)?,
                    crate::opt_parse_col::<Priority>(r, 6)?,
                    r.get::<_, Option<i64>>(7)?,
                    crate::tracker_col(r, 8)?,
                ))
            },
        )?;
        let mut items = Vec::new();
        for (i, row) in rows.enumerate() {
            let (owner, repo, number, title, kind, status, priority, rank, tracker) = row?;
            items.push(HubRankingItem {
                position: i as u32 + 1,
                item_ref: ct_core::item_ref(
                    &ct_core::TrackerRef::new(&owner, &repo, &tracker),
                    number,
                ),
                owner,
                repo,
                number,
                title,
                kind,
                status,
                priority,
                rank,
            });
        }
        let truncated = items.len() > limit as usize;
        items.truncate(limit as usize);
        Ok(HubRanking { items, truncated })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GithubSnapshot, IssueRow, Ledger, NewHubProject, NewProject};
    use ct_core::Priority::*;

    const HUB: &str = "hub-id";

    fn issue(n: i64, state: &str) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("Issue {n}"),
            state: state.into(),
            labels: vec![],
            parent_number: None,
            has_sub_issues: false,
            url: "u".into(),
            updated_at: 1,
            sprints: Vec::new(),
        }
    }

    /// Projetos `o/a` (issues 1–4, a 4 fechada) e `o/b` (issues 1–2).
    fn hub() -> Hub {
        let hub = Hub::open_in_memory().unwrap();
        for (repo, issues) in [
            (
                "a",
                vec![
                    issue(1, "open"),
                    issue(2, "open"),
                    issue(3, "open"),
                    issue(4, "closed"),
                ],
            ),
            ("b", vec![issue(1, "open"), issue(2, "open")]),
        ] {
            hub.add_project(&NewHubProject {
                owner: "o".into(),
                repo: repo.into(),
                name: repo.into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
            let pid = hub.find_project("o", repo).unwrap().unwrap().id;
            hub.apply_github(
                &GithubSnapshot {
                    project_id: pid,
                    issues,
                    prs: vec![],
                    etags: vec![],
                    synced_at: 1,
                },
                HUB,
            )
            .unwrap();
        }
        hub
    }

    fn set(hub: &Hub, repo: &str, n: i64, p: Option<Priority>) -> HubWorkItem {
        hub.set_priority("o", repo, n, p, None, None, HUB).unwrap()
    }

    fn ranks(hub: &Hub, repo: &str) -> Vec<(i64, Option<Priority>, Option<i64>)> {
        let pid = hub.find_project("o", repo).unwrap().unwrap().id;
        hub.list_work_items(pid)
            .unwrap()
            .into_iter()
            .map(|w| (w.gh_number, w.priority, w.rank))
            .collect()
    }

    fn events(hub: &Hub, repo: &str, n: i64, kind: &str) -> Vec<serde_json::Value> {
        let wi = hub.find_work_item("o", repo, n).unwrap().unwrap();
        let mut v: Vec<_> = hub
            .list_events(wi.id, 100)
            .unwrap()
            .into_iter()
            .filter(|e| e.kind == kind)
            .map(|e| {
                assert_eq!(e.env_id, HUB);
                e.payload
            })
            .collect();
        v.reverse();
        v
    }

    fn order(r: &HubRanking) -> Vec<String> {
        r.items
            .iter()
            .map(|i| format!("{}#{}", i.repo, i.number))
            .collect()
    }

    #[test]
    fn definir_e_trocar_prioridade_grava_eventos_e_renumera() {
        let hub = hub();
        // Sem prioridade: não triado, sem work item ainda.
        assert!(hub.find_work_item("o", "a", 1).unwrap().is_none());
        let w = set(&hub, "a", 1, Some(P1));
        assert_eq!((w.priority, w.rank), (Some(P1), Some(1)));
        set(&hub, "a", 2, Some(P1));
        set(&hub, "a", 3, Some(P1));
        assert_eq!(
            ranks(&hub, "a"),
            vec![
                (1, Some(P1), Some(1)),
                (2, Some(P1), Some(2)),
                (3, Some(P1), Some(3))
            ]
        );
        // Sobe para P0: vai para o fim da nova classe e fecha o buraco na antiga.
        let w = hub
            .set_priority("o", "a", 1, Some(P0), Some(" urgente "), None, HUB)
            .unwrap();
        assert_eq!((w.priority, w.rank), (Some(P0), Some(1)));
        assert_eq!(
            ranks(&hub, "a"),
            vec![
                (1, Some(P0), Some(1)),
                (2, Some(P1), Some(1)),
                (3, Some(P1), Some(2))
            ]
        );
        // Mesma prioridade: nada a gravar.
        set(&hub, "a", 1, Some(P0));
        // Tirar a prioridade: volta a não triado, sem rank.
        let w = set(&hub, "a", 2, None);
        assert_eq!((w.priority, w.rank), (None, None));
        assert_eq!(ranks(&hub, "a")[2], (3, Some(P1), Some(1)));

        let ev = events(&hub, "a", 1, "priority_set");
        assert_eq!(ev.len(), 2, "histórico preservado: {ev:?}");
        assert_eq!(
            ev[0],
            json!({ "from": null, "to": "P1", "rank_from": null, "rank_to": 1, "cause": "manual" })
        );
        assert_eq!(
            ev[1],
            json!({ "from": "P1", "to": "P0", "rank_from": 1, "rank_to": 1, "cause": "manual",
                    "reason": "urgente" })
        );
        assert_eq!(events(&hub, "a", 2, "priority_set").len(), 2);
    }

    #[test]
    fn prioridade_por_proposta_e_erros() {
        let hub = hub();
        hub.set_priority("o", "a", 1, Some(P2), None, Some(7), HUB)
            .unwrap();
        let ev = events(&hub, "a", 1, "priority_set");
        assert_eq!(ev[0]["cause"], "proposal");
        assert_eq!(ev[0]["proposal_id"], 7);
        assert!(matches!(
            hub.set_priority("o", "x", 1, Some(P1), None, None, HUB),
            Err(LedgerError::NotFound("projeto", _))
        ));
        assert!(matches!(
            hub.set_priority("o", "a", 99, Some(P1), None, None, HUB),
            Err(LedgerError::NotFound("issue", _))
        ));
    }

    #[test]
    fn item_fora_do_ranking_fica_sem_rank() {
        let hub = hub();
        // Issue fechada no espelho: recebe a prioridade, mas não posição.
        let w = set(&hub, "a", 4, Some(P1));
        assert_eq!((w.priority, w.rank), (Some(P1), None));
        let w = set(&hub, "a", 1, Some(P1));
        assert_eq!(w.rank, Some(1));
        hub.set_status_manual("o", "a", 1, WorkItemStatus::Concluido, None, HUB)
            .unwrap();
        // Concluído sai do ranking; a próxima reescrita da classe zera o rank.
        assert!(order(&hub.ranking(&RankingFilter::default(), 50).unwrap())
            .iter()
            .all(|i| i != "a#1"));
        let w = set(&hub, "a", 2, Some(P1));
        assert_eq!(w.rank, Some(1));
        assert_eq!(ranks(&hub, "a")[0], (1, Some(P1), None));
    }

    #[test]
    fn mover_dentro_da_classe() {
        let hub = hub();
        for n in [1, 2, 3] {
            set(&hub, "a", n, Some(P1));
        }
        let w = hub
            .move_work_item("o", "a", 3, Placement::Before(1), None, HUB)
            .unwrap();
        assert_eq!(w.rank, Some(1));
        assert_eq!(
            ranks(&hub, "a").iter().map(|r| r.2).collect::<Vec<_>>(),
            vec![Some(2), Some(3), Some(1)]
        );
        hub.move_work_item("o", "a", 3, Placement::After(2), Some(9), HUB)
            .unwrap();
        hub.move_work_item("o", "a", 1, Placement::End, None, HUB)
            .unwrap();
        assert_eq!(
            ranks(&hub, "a").iter().map(|r| r.2).collect::<Vec<_>>(),
            vec![Some(3), Some(1), Some(2)]
        );
        let ev = events(&hub, "a", 3, "rank_moved");
        assert_eq!(
            ev,
            vec![
                json!({ "priority": "P1", "rank_from": 3, "rank_to": 1, "cause": "manual" }),
                json!({ "priority": "P1", "rank_from": 1, "rank_to": 3, "cause": "proposal",
                        "proposal_id": 9 }),
            ]
        );
        // Mesma posição: nada a gravar.
        hub.move_work_item("o", "a", 1, Placement::End, None, HUB)
            .unwrap();
        assert_eq!(events(&hub, "a", 1, "rank_moved").len(), 1);

        set(&hub, "b", 1, Some(P1));
        set(&hub, "a", 4, Some(P2));
        // Referência de outra classe, de outro projeto, ou item não triado.
        for (n, to) in [(1, Placement::Before(4)), (1, Placement::After(99))] {
            assert!(matches!(
                hub.move_work_item("o", "a", n, to, None, HUB),
                Err(LedgerError::Conflict(_))
            ));
        }
        hub.set_priority("o", "a", 2, None, None, None, HUB)
            .unwrap();
        assert!(matches!(
            hub.move_work_item("o", "a", 2, Placement::End, None, HUB),
            Err(LedgerError::Conflict(_))
        ));
        assert!(matches!(
            hub.move_work_item("o", "a", 4, Placement::End, None, HUB),
            Err(LedgerError::Conflict(_))
        ));
    }

    #[test]
    fn ranking_do_portfolio_classe_primeiro_e_projeto_desempata() {
        let hub = hub();
        set(&hub, "a", 1, Some(P1));
        set(&hub, "a", 2, Some(P0));
        set(&hub, "b", 1, Some(P1));
        set(&hub, "b", 2, Some(P0));
        let all = hub.ranking(&RankingFilter::default(), 50).unwrap();
        // P0 de qualquer projeto antes de qualquer P1; não triados no fim; a
        // issue fechada fica de fora.
        assert_eq!(order(&all), vec!["a#2", "b#2", "a#1", "b#1", "a#3"]);
        assert_eq!(all.items[4].priority, None);
        assert_eq!(all.items[4].status, WorkItemStatus::Backlog);
        assert_eq!(
            all.items.iter().map(|i| i.position).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );
        assert!(!all.truncated);

        let p = hub
            .move_project("o", "b", Placement::Before(("o", "a")), HUB)
            .unwrap();
        assert_eq!(p.rank, Some(1));
        assert_eq!(hub.find_project("o", "a").unwrap().unwrap().rank, Some(2));
        let all = hub.ranking(&RankingFilter::default(), 50).unwrap();
        assert_eq!(order(&all), vec!["b#2", "a#2", "b#1", "a#1", "a#3"]);
        assert_eq!(
            hub.list_projects()
                .unwrap()
                .iter()
                .map(|p| p.gh_repo.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        let ev: Vec<_> = hub
            .conn
            .prepare("SELECT payload FROM event WHERE type = 'project_rank_moved'")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|p| serde_json::from_str::<serde_json::Value>(&p.unwrap()).unwrap())
            .collect();
        assert_eq!(ev, vec![json!({ "rank_from": null, "rank_to": 1 })]);
        // Mesma posição: nada a gravar.
        hub.move_project("o", "b", Placement::Before(("o", "a")), HUB)
            .unwrap();
        assert!(matches!(
            hub.move_project("o", "b", Placement::After(("o", "zz")), HUB),
            Err(LedgerError::NotFound("projeto", _))
        ));

        let f = RankingFilter {
            project: Some(("O".into(), "A".into())),
            priority: Some(P1),
        };
        assert_eq!(order(&hub.ranking(&f, 50).unwrap()), vec!["a#1"]);
        let cut = hub.ranking(&RankingFilter::default(), 2).unwrap();
        assert_eq!(order(&cut), vec!["b#2", "a#2"]);
        assert!(cut.truncated);
    }

    #[test]
    fn ingestao_nao_toca_na_prioridade_e_triar_nao_e_atividade() {
        let hub = hub();
        let l = Ledger::open_in_memory().unwrap();
        l.add_project(&NewProject {
            name: "a".into(),
            gh_owner: "o".into(),
            gh_repo: "a".into(),
            repo_path: "/a".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        set(&hub, "a", 1, Some(P1));
        let before = hub.tree("o", "a").unwrap().unwrap().roots[0]
            .summary
            .last_activity_at;
        assert_eq!(before, None);
        let w = l.ensure_work_item(1, 1, WorkItemKind::Issue).unwrap();
        l.set_work_item_status(w.id, WorkItemStatus::Bloqueado, "manual")
            .unwrap();
        let b = l.outbox_pending(10_000).unwrap();
        hub.ingest(l.env_id(), "env", &b).unwrap();
        let w = hub.find_work_item("o", "a", 1).unwrap().unwrap();
        assert_eq!(
            (w.priority, w.rank, w.status),
            (Some(P1), Some(1), WorkItemStatus::Bloqueado)
        );
        let t = hub.tree("o", "a").unwrap().unwrap();
        assert_eq!(t.roots[0].work_item.as_ref().unwrap().priority, Some(P1));
        assert!(t.roots[0].summary.last_activity_at.is_some());
        // Padrão: item sem prioridade aparece como não triado (nulo).
        assert_eq!(t.roots[1].work_item, None);
        let d = hub.dashboard().unwrap();
        let wi = &d.projects[0].work_items[0].work_item;
        assert_eq!((wi.priority, wi.rank), (Some(P1), Some(1)));
    }
}
