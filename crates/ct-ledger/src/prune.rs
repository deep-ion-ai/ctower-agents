//! Poda dos dados de execução depois do ack do hub (spec do hub como plano
//! de controle §6; issue #179). O ambiente guarda só o necessário para
//! executar; o hub vira a única cópia do histórico.
//!
//! Um work item é podado quando **tudo** vale:
//!
//! 1. não está ativo: `concluido` (com `closed_at`) **ou** o projeto tem
//!    `unassigned_at`; e nenhuma sessão dele está em `starting`, `running`,
//!    `idle` ou `interrupted` (recuperação pendente conta como ativa);
//! 2. o hub confirmou tudo: o `sync_outbox` está vazio, verificado **dentro**
//!    da transação da poda (e o backfill do ledger anterior à replicação já
//!    entrou no outbox);
//! 3. é velho: `closed_at` (ou `unassigned_at`) e o `last_event_at` da sessão
//!    mais recente são anteriores a `agora - retention_days`;
//! 4. a revisão local não depende dele: nenhuma decisão `autonoma` sem
//!    revisão nele (nem, se for épico, nas sub-issues dele), e ele não é
//!    sub-issue de um épico que ainda tem work item aberto aqui (a revisão do
//!    épico junta as decisões das sub-issues e só existe no ambiente).
//!
//! Os `uid` determinísticos dos eventos do GitHub (`gh:...`) apagados ficam
//! em `pruned_uid` com o maior sufixo usado: uma transição repetida depois
//! da poda (issue reaberta e fechada de novo) recebe o sufixo seguinte, e o
//! hub não a descarta como duplicada.
//!
//! Numa transação `IMMEDIATE`, saem `event`, `decision`, `pr_cache`,
//! `session` e `work_item` do item (com o id marcado em `pruning`, que as
//! triggers append-only aceitam, e removido antes do commit). Projeto
//! desatribuído há mais de `retention_days` e sem work items restantes perde
//! também a linha do cache e o espelho do GitHub (`purging`, migração 3). Os
//! arquivos locais das sessões são do chamador (`ct-sessions`), depois do
//! commit: a poda devolve quais podem sair.

use crate::{project_key, work_item_key, Ledger, LedgerError, Result};
use rusqlite::params;
use serde::Serialize;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Sessão apagada: `id`, `tool_session_id` e `backup_path`.
type SessionFiles = (String, Option<String>, Option<String>);

/// Contador de work items já podados neste ambiente (`ledger_meta`).
const PRUNED_KEY: &str = "pruned_work_items";
/// Quando a última poda apagou algo (`ledger_meta`).
const PRUNED_AT_KEY: &str = "pruned_at";

/// Por que a poda não rodou.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum PruneSkip {
    /// `retention_days = 0`.
    Disabled,
    /// Há mudança no outbox ainda sem ack do hub.
    OutboxPending,
    /// O ledger anterior à replicação ainda não entrou no outbox.
    NotBackfilled,
}

/// Sessão podada e o backup do transcript que pode sair do disco (`None` se
/// não há backup ou se outra sessão restante ainda o usa).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrunedSession {
    pub id: String,
    pub backup_path: Option<String>,
}

/// Resultado de [`Ledger::prune`].
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PruneReport {
    /// `Some` = nada foi verificado nem apagado.
    pub skipped: Option<PruneSkip>,
    /// Work items podados (`owner/repo#n`).
    pub work_items: Vec<String>,
    /// Sessões podadas: os arquivos locais delas saem depois do commit.
    pub sessions: Vec<PrunedSession>,
    /// Projetos desatribuídos removidos do cache (`owner/repo`).
    pub projects: Vec<String>,
    /// Candidatos que não puderam sair (ex.: referência de outro work item),
    /// com o motivo. Ficam para a próxima poda.
    pub failed: Vec<(String, String)>,
}

impl Ledger {
    /// Poda o que o hub já confirmou e passou de `retention_days` em relação
    /// a `now_ms` (veja o módulo). `retention_days = 0` desliga. Precisa
    /// rodar fora de transação (abre uma `IMMEDIATE`).
    pub fn prune(&self, retention_days: u32, now_ms: i64) -> Result<PruneReport> {
        if retention_days == 0 {
            return Ok(PruneReport {
                skipped: Some(PruneSkip::Disabled),
                ..Default::default()
            });
        }
        if !self.conn.is_autocommit() {
            return Err(LedgerError::Conflict(
                "a poda não roda dentro de outra transação".into(),
            ));
        }
        let cutoff = now_ms.saturating_sub(i64::from(retention_days).saturating_mul(DAY_MS));
        self.conn.execute_batch("BEGIN IMMEDIATE")?;
        let r = self.prune_tx(cutoff, now_ms).and_then(|rep| {
            self.conn.execute_batch("COMMIT")?;
            Ok(rep)
        });
        if r.is_err() && !self.conn.is_autocommit() {
            let _ = self.conn.execute_batch("ROLLBACK");
        }
        r
    }

    /// Quantos work items este ambiente já podou (o histórico deles só está
    /// no hub).
    pub fn pruned_work_items(&self) -> Result<i64> {
        Ok(self.meta(PRUNED_KEY)?.unwrap_or(0))
    }

    /// Quando a última poda apagou algo.
    pub fn pruned_at(&self) -> Result<Option<i64>> {
        self.meta(PRUNED_AT_KEY)
    }

    fn outbox_empty(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT NOT EXISTS (SELECT 1 FROM sync_outbox)", [], |r| {
                r.get(0)
            })?)
    }

    fn prune_tx(&self, cutoff: i64, now_ms: i64) -> Result<PruneReport> {
        let mut rep = PruneReport::default();
        // O outbox só esvazia com o ack do hub: vazio aqui (com o lock de
        // escrita) = tudo do work item já está no hub.
        if !self.outbox_empty()? {
            rep.skipped = Some(PruneSkip::OutboxPending);
            return Ok(rep);
        }
        let backfilled: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM ledger_meta WHERE key = 'hub_backfill')",
            [],
            |r| r.get(0),
        )?;
        if !backfilled {
            rep.skipped = Some(PruneSkip::NotBackfilled);
            return Ok(rep);
        }
        // Nenhuma marca sobra de fora desta transação (elas saem antes de
        // cada commit); limpar aqui garante que só as nossas valem.
        self.conn.execute("DELETE FROM pruning", [])?;

        let candidates: Vec<(i64, String, String, i64)> = {
            let mut stmt = self.conn.prepare(
                "SELECT w.id, p.gh_owner, p.gh_repo, w.gh_number
                 FROM work_item w JOIN project p ON p.id = w.project_id
                 WHERE ((w.status = 'concluido' AND w.closed_at IS NOT NULL
                         AND w.closed_at <= ?1)
                        OR (p.unassigned_at IS NOT NULL AND p.unassigned_at <= ?1))
                   AND NOT EXISTS (
                     SELECT 1 FROM session s WHERE s.work_item_id = w.id
                       AND s.state IN ('starting', 'running', 'idle', 'interrupted'))
                   AND coalesce(
                     (SELECT max(s.last_event_at) FROM session s WHERE s.work_item_id = w.id),
                     0) <= ?1
                   AND NOT EXISTS (
                     SELECT 1 FROM decision d
                     WHERE d.origin = 'autonoma' AND d.reviewed_at IS NULL
                       AND (d.work_item_id = w.id OR d.work_item_id IN (
                         SELECT sw.id FROM work_item sw JOIN issue_cache ic
                           ON ic.project_id = sw.project_id AND ic.gh_number = sw.gh_number
                         WHERE sw.project_id = w.project_id
                           AND ic.parent_number = w.gh_number)))
                   AND NOT EXISTS (
                     SELECT 1 FROM issue_cache ic JOIN work_item pw
                       ON pw.project_id = ic.project_id AND pw.gh_number = ic.parent_number
                     WHERE ic.project_id = w.project_id AND ic.gh_number = w.gh_number
                       AND pw.status <> 'concluido')
                 ORDER BY w.id",
            )?;
            let rows = stmt.query_map([cutoff], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };

        let mut pruned_sessions: Vec<SessionFiles> = Vec::new();
        let mut pull_etags: Vec<(String, String)> = Vec::new();
        for (id, owner, repo, number) in candidates {
            let key = work_item_key(&owner, &repo, number);
            match self.savepoint("prune_item", || self.prune_work_item(id)) {
                Ok(sessions) => {
                    pruned_sessions.extend(sessions);
                    if !pull_etags.contains(&(owner.clone(), repo.clone())) {
                        pull_etags.push((owner, repo));
                    }
                    rep.work_items.push(key);
                }
                Err(e) => rep.failed.push((key, e.to_string())),
            }
        }
        // O ETag do PR de uma branch podada faria um work item recriado
        // (issue reaberta) receber `304` e perder o PR: os dos PRs do
        // projeto saem e a próxima busca vem completa.
        for (owner, repo) in &pull_etags {
            self.delete_etags(&format!("/repos/{owner}/{repo}/pulls?"))?;
        }

        let orphans: Vec<(i64, String, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT p.id, p.gh_owner, p.gh_repo FROM project p
                 WHERE p.unassigned_at IS NOT NULL AND p.unassigned_at <= ?1
                   AND NOT EXISTS (SELECT 1 FROM work_item w WHERE w.project_id = p.id)
                 ORDER BY p.id",
            )?;
            let rows = stmt.query_map([cutoff], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (id, owner, repo) in orphans {
            let key = project_key(&owner, &repo);
            match self.savepoint("prune_project", || self.drop_project(id, &owner, &repo)) {
                Ok(()) => rep.projects.push(key),
                Err(e) => rep.failed.push((key, e.to_string())),
            }
        }

        // Backup de transcript que outra sessão restante ainda usa (mesmo
        // caminho ou o mesmo `tool_session_id`, que dá o nome do arquivo)
        // fica no disco.
        for (id, tool_session_id, backup) in pruned_sessions {
            let backup = match backup {
                Some(b) if !self.backup_in_use(&b, tool_session_id.as_deref())? => Some(b),
                _ => None,
            };
            rep.sessions.push(PrunedSession {
                id,
                backup_path: backup,
            });
        }

        // Defesa: nenhuma marca fica e nada entrou no outbox durante a poda.
        let marks: i64 = self.conn.query_row(
            "SELECT (SELECT count(*) FROM pruning) + (SELECT count(*) FROM purging)",
            [],
            |r| r.get(0),
        )?;
        if marks != 0 || !self.outbox_empty()? {
            return Err(LedgerError::Corrupt(
                "poda deixaria marcas em pruning/purging ou mudanças no outbox".into(),
            ));
        }
        if !rep.work_items.is_empty() || !rep.projects.is_empty() {
            self.conn.execute(
                "INSERT INTO ledger_meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = value + excluded.value",
                params![PRUNED_KEY, rep.work_items.len() as i64],
            )?;
            self.conn.execute(
                "INSERT INTO ledger_meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![PRUNED_AT_KEY, now_ms],
            )?;
        }
        Ok(rep)
    }

    /// Roda `f` num SAVEPOINT próprio: um candidato que falha (ex.: FK de
    /// outro work item) é desfeito sozinho, sem derrubar a poda inteira.
    fn savepoint<T>(&self, name: &str, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.conn.execute_batch(&format!("SAVEPOINT {name}"))?;
        match f() {
            Ok(v) => {
                self.conn.execute_batch(&format!("RELEASE {name}"))?;
                Ok(v)
            }
            Err(e) => {
                let _ = self
                    .conn
                    .execute_batch(&format!("ROLLBACK TO {name}; RELEASE {name}"));
                Err(e)
            }
        }
    }

    /// Apaga as linhas do work item; devolve as sessões apagadas
    /// (`id`, `tool_session_id`, `backup_path`).
    fn prune_work_item(&self, id: i64) -> Result<Vec<SessionFiles>> {
        let sessions: Vec<SessionFiles> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, tool_session_id, backup_path FROM session
                 WHERE work_item_id = ?1 ORDER BY started_at",
            )?;
            let rows = stmt.query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        self.conn
            .execute("INSERT INTO pruning (work_item_id) VALUES (?1)", [id])?;
        self.keep_gh_uids("SELECT uid FROM event WHERE work_item_id = ?1", id)?;
        for sql in [
            "DELETE FROM event WHERE work_item_id = ?1",
            "DELETE FROM decision WHERE work_item_id = ?1",
            "DELETE FROM pr_cache WHERE work_item_id = ?1",
            "DELETE FROM session WHERE work_item_id = ?1",
            "DELETE FROM work_item WHERE id = ?1",
        ] {
            self.conn.execute(sql, [id])?;
        }
        self.conn
            .execute("DELETE FROM pruning WHERE work_item_id = ?1", [id])?;
        Ok(sessions)
    }

    /// Projeto desatribuído sem work items: sai a linha do cache e o espelho
    /// do GitHub dele (eventos de projeto pela exceção `purging`).
    fn drop_project(&self, id: i64, owner: &str, repo: &str) -> Result<()> {
        self.conn
            .execute("INSERT INTO purging (project_id) VALUES (?1)", [id])?;
        self.keep_gh_uids("SELECT uid FROM event WHERE project_id = ?1", id)?;
        for sql in [
            "DELETE FROM event WHERE project_id = ?1",
            "DELETE FROM pr_cache WHERE project_id = ?1",
            "DELETE FROM issue_cache WHERE project_id = ?1",
            "DELETE FROM sync_state WHERE project_id = ?1",
            "DELETE FROM project WHERE id = ?1",
        ] {
            self.conn.execute(sql, [id])?;
        }
        self.conn
            .execute("DELETE FROM purging WHERE project_id = ?1", [id])?;
        self.delete_etags(&format!("/repos/{owner}/{repo}/"))
    }

    /// Guarda em `pruned_uid` o maior sufixo de cada `uid` determinístico
    /// (`gh:...`, `gh:...:<n>`) que `sql` (com o id em `?1`) vai apagar.
    fn keep_gh_uids(&self, sql: &str, id: i64) -> Result<()> {
        let uids: Vec<String> = {
            let mut stmt = self.conn.prepare(sql)?;
            let rows = stmt.query_map([id], |r| r.get::<_, Option<String>>(0))?;
            rows.filter_map(|r| r.transpose())
                .collect::<rusqlite::Result<_>>()?
        };
        for uid in uids.iter().filter(|u| u.starts_with("gh:")) {
            let (base, n) = split_uid_suffix(uid);
            self.conn.execute(
                "INSERT INTO pruned_uid (base, max_n) VALUES (?1, ?2)
                 ON CONFLICT (base) DO UPDATE SET max_n = max(max_n, excluded.max_n)",
                params![base, n],
            )?;
        }
        Ok(())
    }

    /// ETags cujo caminho começa com `prefix` (sem `LIKE`: `_` e `%` são
    /// válidos em nomes de repositório).
    fn delete_etags(&self, prefix: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM gh_etag WHERE substr(key, 1, length(?1)) = ?1",
            [prefix],
        )?;
        Ok(())
    }

    fn backup_in_use(&self, backup: &str, tool_session_id: Option<&str>) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM session
                            WHERE backup_path = ?1 OR (?2 IS NOT NULL AND tool_session_id = ?2))",
            params![backup, tool_session_id],
            |r| r.get(0),
        )?)
    }
}

/// `base:<n>` → `(base, n)`; sem sufixo numérico, `(uid, 1)` (o primeiro
/// uso de uma base não tem sufixo; os seguintes são `:2`, `:3`...). As
/// bases `gh:...` terminam no estado (`:closed`, `:merged`...), nunca em
/// número.
pub(crate) fn split_uid_suffix(uid: &str) -> (&str, i64) {
    if let Some((base, raw)) = uid.rsplit_once(':') {
        if let Ok(n) = raw.parse::<i64>() {
            if n >= 2 && !raw.starts_with(['0', '+']) {
                return (base, n);
            }
        }
    }
    (uid, 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ledger_with_project;
    use crate::{
        ConfigResponse, GithubSnapshot, IssueRow, NewDecision, NewEvent, NewSession, PrRow,
    };
    use ct_core::{
        now_ms, DecisionOrigin, PrState, SessionMode, SessionState, Tool, WorkItemKind,
        WorkItemStatus,
    };

    const OLD: i64 = 31 * DAY_MS;

    fn count(l: &Ledger, sql: &str) -> i64 {
        l.conn.query_row(sql, [], |r| r.get(0)).unwrap()
    }

    /// Linhas do work item em cada tabela podada.
    fn rows_of(l: &Ledger, wi: i64) -> [i64; 5] {
        let c = |t: &str, col: &str| {
            l.conn
                .query_row(
                    &format!("SELECT count(*) FROM {t} WHERE {col} = ?1"),
                    [wi],
                    |r| r.get(0),
                )
                .unwrap()
        };
        [
            c("event", "work_item_id"),
            c("decision", "work_item_id"),
            c("pr_cache", "work_item_id"),
            c("session", "work_item_id"),
            c("work_item", "id"),
        ]
    }

    /// Work item com sessão (terminada em `end`), decisão, backup de
    /// transcript e PR no espelho, com o status dado.
    fn item(l: &Ledger, project: i64, n: i64, end: SessionState) -> (i64, String) {
        let wi = l.ensure_work_item(project, n, WorkItemKind::Issue).unwrap();
        l.set_work_item_branch(wi.id, &format!("feat/{n}"), "/w")
            .unwrap();
        let s = l
            .create_session(&NewSession {
                work_item_id: wi.id,
                tool: Tool::Claude,
                cwd: "/w".into(),
                launch_cmd: vec![],
                model: None,
                mode: SessionMode::Native,
            })
            .unwrap();
        l.set_session_tool_info(&s.id, &format!("tool-{n}"), "/t.jsonl")
            .unwrap();
        l.set_session_backup_path(&s.id, &format!("/data/transcripts/tool-{n}.jsonl"))
            .unwrap();
        if end != SessionState::Starting {
            l.transition_session(&s.id, SessionState::Running, None)
                .unwrap();
            if end != SessionState::Running {
                l.transition_session(&s.id, end, Some(0)).unwrap();
            }
        }
        l.record_decision(&NewDecision {
            work_item_id: wi.id,
            session_id: Some(s.id.clone()),
            // Já revisada: decisão autônoma pendente impede a poda (teste próprio).
            origin: DecisionOrigin::Orientada,
            title: "t".into(),
            context: "c".into(),
            decision: "d".into(),
            consequences: None,
        })
        .unwrap();
        l.apply_github(&GithubSnapshot {
            project_id: project,
            issues: vec![],
            prs: vec![PrRow {
                number: 100 + n,
                work_item_id: wi.id,
                head: format!("feat/{n}"),
                base: "develop".into(),
                state: PrState::Open,
                url: "u".into(),
                updated_at: 0,
                head_sha: None,
            }],
            etags: vec![(
                format!("/repos/deep-ion-ai/demo/pulls?state=all&head=x{n}"),
                "e".into(),
            )],
            synced_at: now_ms(),
        })
        .unwrap();
        (wi.id, s.id)
    }

    /// Envelhece o work item: fechado (se `concluido`) e última atividade de
    /// sessão há `age` ms.
    fn age(l: &Ledger, wi: i64, age: i64) {
        let t = now_ms() - age;
        l.conn
            .execute(
                "UPDATE work_item SET closed_at = CASE WHEN status = 'concluido' THEN ?2 END
                 WHERE id = ?1",
                params![wi, t],
            )
            .unwrap();
        l.conn
            .execute(
                "UPDATE session SET last_event_at = ?2 WHERE work_item_id = ?1",
                params![wi, t],
            )
            .unwrap();
    }

    /// Ledger com o backfill feito e o outbox confirmado.
    fn setup() -> (Ledger, i64) {
        let (l, p) = ledger_with_project();
        l.ensure_backfilled().unwrap();
        (l, p.id)
    }

    fn ack(l: &Ledger) {
        l.outbox_ack(i64::MAX).unwrap();
    }

    fn close(l: &Ledger, wi: i64) {
        l.set_work_item_status(wi, WorkItemStatus::Concluido, "manual")
            .unwrap();
    }

    #[test]
    fn fechado_confirmado_e_velho_e_podado() {
        let (l, p) = setup();
        let (velho, sv) = item(&l, p, 1, SessionState::Exited);
        let (ativo, _) = item(&l, p, 2, SessionState::Exited);
        close(&l, velho);
        age(&l, velho, OLD);
        age(&l, ativo, OLD);
        l.conn
            .execute(
                "INSERT INTO gh_etag (key, etag, updated_at)
                 VALUES ('/repos/deep-ion-ai/demo/issues?state=open', 'i', 0)",
                [],
            )
            .unwrap();
        ack(&l);

        let r = l.prune(30, now_ms()).unwrap();
        assert_eq!(r.skipped, None);
        assert_eq!(r.work_items, vec!["deep-ion-ai/demo#1"]);
        assert!(r.failed.is_empty() && r.projects.is_empty());
        assert_eq!(
            r.sessions,
            vec![PrunedSession {
                id: sv,
                backup_path: Some("/data/transcripts/tool-1.jsonl".into()),
            }]
        );
        assert_eq!(rows_of(&l, velho), [0; 5]);
        assert!(rows_of(&l, ativo).iter().all(|n| *n > 0));
        // Os ETags dos PRs saem (um item recriado não pode receber `304`); o
        // das issues fica. O projeto atribuído fica.
        assert_eq!(count(&l, "SELECT count(*) FROM gh_etag"), 1);
        assert!(l.project_active(p).unwrap());
        assert_eq!(l.pruned_work_items().unwrap(), 1);
        assert!(l.pruned_at().unwrap().is_some());
        // Nada no outbox, nenhuma marca.
        assert_eq!(l.outbox_stats().unwrap().pending, 0);
        assert_eq!(count(&l, "SELECT count(*) FROM pruning"), 0);

        // De novo: nada a podar, o contador não muda.
        let r = l.prune(30, now_ms()).unwrap();
        assert!(r.work_items.is_empty());
        assert_eq!(l.pruned_work_items().unwrap(), 1);

        // Issue reaberta: a chave natural cria uma linha nova, que vai ao hub.
        let wi = l.ensure_work_item(p, 1, WorkItemKind::Issue).unwrap();
        assert_ne!(wi.id, velho);
        assert!(l.outbox_stats().unwrap().pending > 0);
    }

    #[test]
    fn item_ativo_nao_e_podado() {
        let (l, p) = setup();
        let mut ids = Vec::new();
        for (n, st) in [
            (1, WorkItemStatus::EmExecucao),
            (2, WorkItemStatus::Bloqueado),
            (3, WorkItemStatus::Pausado),
            (4, WorkItemStatus::Backlog),
            (5, WorkItemStatus::EmRevisao),
            (6, WorkItemStatus::AguardandoVoce),
        ] {
            let (wi, _) = item(&l, p, n, SessionState::Exited);
            l.set_work_item_status(wi, st, "manual").unwrap();
            age(&l, wi, 10 * OLD);
            ids.push(wi);
        }
        ack(&l);
        let r = l.prune(30, now_ms()).unwrap();
        assert!(r.work_items.is_empty(), "{r:?}");
        for wi in ids {
            assert!(rows_of(&l, wi).iter().all(|n| *n > 0));
        }
    }

    #[test]
    fn sessao_interrompida_ou_viva_impede_a_poda() {
        let (l, p) = setup();
        let mut ids = Vec::new();
        for (n, end) in [
            (1, SessionState::Interrupted),
            (2, SessionState::Running),
            (3, SessionState::Idle),
            (4, SessionState::Starting),
        ] {
            let (wi, _) = item(&l, p, n, end);
            close(&l, wi);
            age(&l, wi, OLD);
            ids.push(wi);
        }
        ack(&l);
        assert!(l.prune(30, now_ms()).unwrap().work_items.is_empty());
        for wi in ids {
            assert_eq!(rows_of(&l, wi)[4], 1);
        }
    }

    #[test]
    fn outbox_pendente_impede_a_poda() {
        let (l, p) = setup();
        let (wi, _) = item(&l, p, 1, SessionState::Exited);
        close(&l, wi);
        age(&l, wi, OLD);
        // Tudo confirmado menos a última mudança.
        let last: i64 = count(&l, "SELECT max(seq) FROM sync_outbox");
        l.outbox_ack(last - 1).unwrap();
        let antes = rows_of(&l, wi);
        let r = l.prune(30, now_ms()).unwrap();
        assert_eq!(r.skipped, Some(PruneSkip::OutboxPending));
        assert_eq!(rows_of(&l, wi), antes);
        assert_eq!(l.pruned_work_items().unwrap(), 0);
        ack(&l);
        assert_eq!(l.prune(30, now_ms()).unwrap().work_items.len(), 1);
    }

    #[test]
    fn sem_backfill_nada_e_podado() {
        let (l, p) = ledger_with_project();
        let (wi, _) = item(&l, p.id, 1, SessionState::Exited);
        close(&l, wi);
        age(&l, wi, OLD);
        ack(&l);
        let r = l.prune(30, now_ms()).unwrap();
        assert_eq!(r.skipped, Some(PruneSkip::NotBackfilled));
        assert_eq!(rows_of(&l, wi)[4], 1);
    }

    #[test]
    fn recente_nao_e_podado_e_zero_desliga() {
        let (l, p) = setup();
        // Fechado há muito, mas com sessão ativa há pouco.
        let (a, _) = item(&l, p, 1, SessionState::Exited);
        close(&l, a);
        age(&l, a, OLD);
        l.conn
            .execute(
                "UPDATE session SET last_event_at = ?2 WHERE work_item_id = ?1",
                params![a, now_ms() - DAY_MS],
            )
            .unwrap();
        // Fechado há pouco.
        let (b, _) = item(&l, p, 2, SessionState::Exited);
        close(&l, b);
        age(&l, b, 29 * DAY_MS);
        // Velho de verdade, mas a poda está desligada.
        let (c, _) = item(&l, p, 3, SessionState::Exited);
        close(&l, c);
        age(&l, c, 10 * OLD);
        ack(&l);

        let r = l.prune(0, now_ms()).unwrap();
        assert_eq!(r.skipped, Some(PruneSkip::Disabled));
        assert_eq!(rows_of(&l, c)[4], 1);

        let r = l.prune(30, now_ms()).unwrap();
        assert_eq!(r.work_items, vec!["deep-ion-ai/demo#3"]);
        assert_eq!((rows_of(&l, a)[4], rows_of(&l, b)[4]), (1, 1));
        // Retenção maior: nada.
        let (d, _) = item(&l, p, 4, SessionState::Exited);
        close(&l, d);
        age(&l, d, OLD);
        ack(&l);
        assert!(l.prune(60, now_ms()).unwrap().work_items.is_empty());
    }

    fn unassign_all(l: &Ledger) {
        l.apply_hub_config(
            &ConfigResponse {
                config_rev: 1,
                config_epoch: 1,
                changed: true,
                projects: Some(vec![]),
                removed: Some(vec![]),
            },
            |_| String::new(),
        )
        .unwrap();
    }

    #[test]
    fn desatribuido_e_podado_apos_o_prazo_e_some_do_cache() {
        let (l, p) = setup();
        // Em execução (sem sessão viva) num projeto que o hub desatribuiu.
        let (wi, _) = item(&l, p, 1, SessionState::Exited);
        age(&l, wi, OLD);
        l.append_event(NewEvent {
            project_id: p,
            work_item_id: None,
            session_id: None,
            kind: "projeto",
            payload: serde_json::json!({}),
        })
        .unwrap();
        l.apply_github(&GithubSnapshot {
            project_id: p,
            issues: vec![IssueRow {
                gh_number: 1,
                title: "i".into(),
                state: "open".into(),
                labels: vec![],
                parent_number: None,
                has_sub_issues: false,
                url: "u".into(),
                updated_at: 0,
                ..Default::default()
            }],
            prs: vec![],
            etags: vec![("/repos/deep-ion-ai/demo/issues?x".into(), "e".into())],
            synced_at: now_ms(),
        })
        .unwrap();
        l.conn
            .execute(
                "INSERT INTO gh_etag (key, etag, updated_at)
                 VALUES ('/repos/deep-ion-ai/demo-2/issues?x', 'outro', 0)",
                [],
            )
            .unwrap();
        unassign_all(&l);
        ack(&l);

        // Desatribuído agora: ainda no prazo.
        assert!(l.prune(30, now_ms()).unwrap().work_items.is_empty());
        assert_eq!(rows_of(&l, wi)[4], 1);

        l.conn
            .execute(
                "UPDATE project SET unassigned_at = ?2 WHERE id = ?1",
                params![p, now_ms() - OLD],
            )
            .unwrap();
        let r = l.prune(30, now_ms()).unwrap();
        assert_eq!(r.work_items, vec!["deep-ion-ai/demo#1"]);
        assert_eq!(r.projects, vec!["deep-ion-ai/demo"]);
        assert_eq!(rows_of(&l, wi), [0; 5]);
        for t in ["project", "issue_cache", "sync_state", "event", "pr_cache"] {
            assert_eq!(count(&l, &format!("SELECT count(*) FROM {t}")), 0, "{t}");
        }
        // Só os ETags do projeto saem (`demo-2` tem o mesmo prefixo de texto).
        let keys: Vec<String> = l
            .conn
            .prepare("SELECT key FROM gh_etag")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(keys, vec!["/repos/deep-ion-ai/demo-2/issues?x"]);
        assert_eq!(count(&l, "SELECT count(*) FROM purging"), 0);
        assert_eq!(l.outbox_stats().unwrap().pending, 0);
    }

    #[test]
    fn desatribuido_com_sessao_interrompida_fica() {
        let (l, p) = setup();
        let (wi, _) = item(&l, p, 1, SessionState::Interrupted);
        age(&l, wi, OLD);
        unassign_all(&l);
        l.conn
            .execute(
                "UPDATE project SET unassigned_at = ?2 WHERE id = ?1",
                params![p, now_ms() - OLD],
            )
            .unwrap();
        ack(&l);
        let r = l.prune(30, now_ms()).unwrap();
        assert!(r.work_items.is_empty() && r.projects.is_empty());
        assert_eq!(count(&l, "SELECT count(*) FROM project"), 1);
    }

    #[test]
    fn backup_usado_por_sessao_restante_fica_no_disco() {
        let (l, p) = setup();
        let (velho, sv) = item(&l, p, 1, SessionState::Exited);
        let (_, sa) = item(&l, p, 2, SessionState::Exited);
        // A sessão restante usa o mesmo `tool_session_id` (retomada).
        l.set_session_tool_info(&sa, "tool-1", "/t.jsonl").unwrap();
        close(&l, velho);
        age(&l, velho, OLD);
        ack(&l);
        let r = l.prune(30, now_ms()).unwrap();
        assert_eq!(
            r.sessions,
            vec![PrunedSession {
                id: sv,
                backup_path: None,
            }]
        );
    }

    /// Decisão de outro work item que aponta a sessão do candidato: a FK
    /// impede, o candidato é desfeito sozinho e os demais saem.
    #[test]
    fn candidato_com_referencia_externa_falha_sozinho() {
        let (l, p) = setup();
        let (a, sa) = item(&l, p, 1, SessionState::Exited);
        let (b, _) = item(&l, p, 2, SessionState::Exited);
        let (c, _) = item(&l, p, 3, SessionState::Exited);
        l.record_decision(&NewDecision {
            work_item_id: c,
            session_id: Some(sa),
            origin: DecisionOrigin::Autonoma,
            title: "x".into(),
            context: "c".into(),
            decision: "d".into(),
            consequences: None,
        })
        .unwrap();
        for wi in [a, b] {
            close(&l, wi);
            age(&l, wi, OLD);
        }
        ack(&l);
        let antes = rows_of(&l, a);
        let r = l.prune(30, now_ms()).unwrap();
        assert_eq!(r.work_items, vec!["deep-ion-ai/demo#2"]);
        assert_eq!(r.failed.len(), 1);
        assert_eq!(r.failed[0].0, "deep-ion-ai/demo#1");
        assert_eq!(rows_of(&l, a), antes);
        assert_eq!(rows_of(&l, b), [0; 5]);
        assert_eq!(count(&l, "SELECT count(*) FROM pruning"), 0);
        assert_eq!(l.pruned_work_items().unwrap(), 1);
    }

    /// Fora da transação da poda, `event` e `decision` continuam
    /// append-only; uma marca em `pruning` libera só as linhas daquele work
    /// item, nunca as sem work item.
    #[test]
    fn triggers_bloqueiam_delete_fora_da_poda() {
        let (l, p) = setup();
        let (a, _) = item(&l, p, 1, SessionState::Exited);
        let (b, _) = item(&l, p, 2, SessionState::Exited);
        l.append_event(NewEvent {
            project_id: p,
            work_item_id: None,
            session_id: None,
            kind: "projeto",
            payload: serde_json::json!({}),
        })
        .unwrap();
        assert!(l.conn.execute("DELETE FROM event", []).is_err());
        assert!(l.conn.execute("DELETE FROM decision", []).is_err());
        assert!(l
            .conn
            .execute("DELETE FROM event WHERE work_item_id IS NULL", [])
            .is_err());

        l.conn
            .execute("INSERT INTO pruning (work_item_id) VALUES (?1)", [a])
            .unwrap();
        assert!(l
            .conn
            .execute("DELETE FROM event WHERE work_item_id = ?1", [b])
            .is_err());
        assert!(l
            .conn
            .execute("DELETE FROM decision WHERE work_item_id = ?1", [b])
            .is_err());
        assert!(l
            .conn
            .execute("DELETE FROM event WHERE work_item_id IS NULL", [])
            .is_err());
        assert!(l
            .conn
            .execute("UPDATE event SET type = 'x' WHERE work_item_id = ?1", [a])
            .is_err());
        l.conn
            .execute("DELETE FROM decision WHERE work_item_id = ?1", [a])
            .unwrap();
        l.conn
            .execute("DELETE FROM event WHERE work_item_id = ?1", [a])
            .unwrap();
    }

    #[test]
    fn poda_recusa_rodar_dentro_de_transacao() {
        let (l, _) = setup();
        l.conn.execute_batch("BEGIN").unwrap();
        assert!(matches!(
            l.prune(30, now_ms()),
            Err(LedgerError::Conflict(_))
        ));
        l.conn.execute_batch("ROLLBACK").unwrap();
    }

    fn autonoma(l: &Ledger, wi: i64) -> i64 {
        l.record_decision(&NewDecision {
            work_item_id: wi,
            session_id: None,
            origin: DecisionOrigin::Autonoma,
            title: "a".into(),
            context: "c".into(),
            decision: "d".into(),
            consequences: None,
        })
        .unwrap()
        .id
    }

    fn issue(n: i64, state: &str, parent: Option<i64>) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("#{n}"),
            state: state.into(),
            labels: vec![],
            parent_number: parent,
            has_sub_issues: parent.is_none(),
            url: "u".into(),
            updated_at: 0,
            ..Default::default()
        }
    }

    fn github(l: &Ledger, p: i64, issues: Vec<IssueRow>) {
        l.apply_github(&GithubSnapshot {
            project_id: p,
            issues,
            prs: vec![],
            etags: vec![],
            synced_at: now_ms(),
        })
        .unwrap();
    }

    /// A revisão (`review.get`, `review.mark_reviewed`) só existe no
    /// ambiente: decisão autônoma pendente segura o work item.
    #[test]
    fn decisao_autonoma_pendente_impede_a_poda() {
        let (l, p) = setup();
        let (wi, _) = item(&l, p, 1, SessionState::Exited);
        let d = autonoma(&l, wi);
        close(&l, wi);
        age(&l, wi, OLD);
        ack(&l);
        assert!(l.prune(30, now_ms()).unwrap().work_items.is_empty());
        assert_eq!(rows_of(&l, wi)[4], 1);

        l.mark_reviewed(&[d], Some(7)).unwrap();
        ack(&l);
        assert_eq!(l.prune(30, now_ms()).unwrap().work_items.len(), 1);
    }

    /// Sub-issue de épico ainda aberto aqui fica (a revisão do épico junta
    /// as decisões dela); épico com decisão pendente numa sub-issue também.
    #[test]
    fn sub_issue_de_epico_ativo_e_epico_com_pendencia_na_sub_ficam() {
        let (l, p) = setup();
        let epic = l.ensure_work_item(p, 1, WorkItemKind::Epic).unwrap().id;
        let (sub, _) = item(&l, p, 2, SessionState::Exited);
        let (outra, _) = item(&l, p, 3, SessionState::Exited);
        github(
            &l,
            p,
            vec![
                issue(1, "open", None),
                issue(2, "open", Some(1)),
                issue(3, "open", Some(1)),
            ],
        );
        close(&l, sub);
        age(&l, sub, OLD);
        let d = autonoma(&l, outra);
        ack(&l);
        // Épico em aberto: a sub-issue fechada e velha fica.
        assert!(l.prune(30, now_ms()).unwrap().work_items.is_empty());
        assert_eq!(rows_of(&l, sub)[4], 1);

        // Épico fechado (e velho): a sub-issue sai; o épico fica pela
        // decisão pendente na outra sub-issue (ainda aberta).
        close(&l, epic);
        age(&l, epic, OLD);
        ack(&l);
        let r = l.prune(30, now_ms()).unwrap();
        assert_eq!(r.work_items, vec!["deep-ion-ai/demo#2"]);
        assert!(l.get_work_item(epic).is_ok());

        // Revisada a decisão (e a outra sub-issue fechada e velha), saem os dois.
        l.mark_reviewed(&[d], Some(9)).unwrap();
        close(&l, outra);
        age(&l, outra, OLD);
        ack(&l);
        let mut r = l.prune(30, now_ms()).unwrap().work_items;
        r.sort();
        assert_eq!(r, vec!["deep-ion-ai/demo#1", "deep-ion-ai/demo#3"]);
    }

    #[test]
    fn sufixo_de_uid() {
        assert_eq!(
            split_uid_suffix("gh:issue:o/r#1:closed"),
            ("gh:issue:o/r#1:closed", 1)
        );
        assert_eq!(
            split_uid_suffix("gh:issue:o/r#1:closed:3"),
            ("gh:issue:o/r#1:closed", 3)
        );
        assert_eq!(split_uid_suffix("gh:x:1"), ("gh:x:1", 1));
        assert_eq!(split_uid_suffix("gh:x:02"), ("gh:x:02", 1));
    }

    /// Issue fechada → podada → reaberta → fechada de novo: o evento novo
    /// não reusa o `uid` que o hub já tem (`ON CONFLICT (uid) DO NOTHING` o
    /// descartaria).
    #[test]
    fn uid_do_github_nao_e_reusado_depois_da_poda() {
        let (l, p) = setup();
        let base = crate::gh_issue_closed_uid("deep-ion-ai", "demo", 1);
        let closed_uids = |l: &Ledger| -> Vec<String> {
            l.conn
                .prepare("SELECT uid FROM event WHERE type = 'issue_closed' ORDER BY id")
                .unwrap()
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap()
        };
        let fecha = |l: &Ledger| -> i64 {
            let wi = l.ensure_work_item(p, 1, WorkItemKind::Issue).unwrap().id;
            github(l, p, vec![issue(1, "open", None)]);
            github(l, p, vec![issue(1, "closed", None)]);
            wi
        };

        let wi = fecha(&l);
        assert_eq!(closed_uids(&l), vec![base.clone()]);
        // Duas vezes antes da poda: `:2` (o comportamento de antes).
        github(&l, p, vec![issue(1, "open", None)]);
        github(&l, p, vec![issue(1, "closed", None)]);
        assert_eq!(closed_uids(&l), vec![base.clone(), format!("{base}:2")]);
        close(&l, wi);
        age(&l, wi, OLD);
        ack(&l);
        assert_eq!(l.prune(30, now_ms()).unwrap().work_items.len(), 1);
        assert!(closed_uids(&l).is_empty());

        // Reaberta e fechada de novo: `:3`, nunca um `uid` que o hub já tem.
        let wi = fecha(&l);
        assert_eq!(closed_uids(&l), vec![format!("{base}:3")]);

        // Podada de novo: a próxima é `:4`.
        close(&l, wi);
        age(&l, wi, OLD);
        ack(&l);
        assert_eq!(l.prune(30, now_ms()).unwrap().work_items.len(), 1);
        fecha(&l);
        assert_eq!(closed_uids(&l), vec![format!("{base}:4")]);
    }
}
