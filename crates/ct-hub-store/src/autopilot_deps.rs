//! Dependências entre issues do piloto automático (#215, épico #178),
//! guardadas no hub (migração 43), sem relação nativa no rastreador.
//!
//! - **Arestas** ([`DependencyEdge`]): `number` é bloqueada por `blocker`.
//!   A sessão de análise grava as de origem `analysis`; a pessoa corrige na
//!   UI com as de origem `manual`: adicionar grava uma manual ativa, remover
//!   grava uma manual com `removed` (lápide). A análise seguinte substitui
//!   só as `analysis` da issue e nunca passa por cima de uma manual.
//! - **Rodada de análise** ([`AnalysisRound`]): uma por projeto enquanto a
//!   janela da política está aberta, com a sessão, as issues enviadas a ela
//!   (com a impressão digital do momento, [`issue_fingerprint`]) e quando o
//!   planejamento terminou. Fechar a janela apaga a rodada; as arestas e as
//!   issues analisadas ficam para a próxima.
//! - **Issues analisadas**: a impressão digital de cada issue na última
//!   análise que a incluiu. Impressão diferente = issue nova ou alterada, que
//!   espera a reanálise.
//!
//! A decisão fica em `cthubd::autopilot`; aqui só se guarda.

use super::Hub;
use crate::{LedgerError, Result};
use ct_core::SessionState;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// Origem da aresta gravada pela sessão de análise.
pub const DEP_SOURCE_ANALYSIS: &str = "analysis";
/// Origem da aresta corrigida na UI: prevalece sobre a análise.
pub const DEP_SOURCE_MANUAL: &str = "manual";

/// Justificativa de uma aresta: no máximo isto (o resto é cortado).
pub const DEP_REASON_MAX_CHARS: usize = 500;
/// Issues por gravação da análise.
pub const DEP_MAX_ITEMS: usize = 500;
/// Bloqueantes por issue.
pub const DEP_MAX_BLOCKERS: usize = 50;

/// Uma aresta: `number` é bloqueada por `blocker`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct DependencyEdge {
    pub number: i64,
    pub blocker: i64,
    /// Justificativa curta (da análise ou da pessoa).
    pub reason: String,
    /// `analysis` ou `manual`.
    pub source: String,
    /// Manual removida: a lápide que impede a análise de recriar a aresta.
    pub removed: bool,
    pub updated_at: i64,
}

/// Um bloqueante informado pela sessão de análise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct BlockerInput {
    pub number: i64,
    #[serde(default)]
    pub reason: String,
}

/// As dependências de uma issue informadas pela sessão de análise.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DependencyInput {
    pub number: i64,
    #[serde(default)]
    pub blocked_by: Vec<BlockerInput>,
}

/// O que a sessão de análise grava (`autopilot_set_dependencies`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct DependencySet {
    #[serde(default)]
    pub items: Vec<DependencyInput>,
    /// Ordem de execução sugerida (números); vazia = mantém a anterior.
    #[serde(default)]
    pub order: Vec<i64>,
}

/// Resultado de [`Hub::set_dependencies_from_analysis`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DependencySetResult {
    /// Issues com as dependências substituídas.
    pub items: usize,
    /// Arestas de análise gravadas.
    pub edges: usize,
    /// Arestas ignoradas porque a pessoa corrigiu a mesma à mão.
    pub kept_manual: usize,
    /// Issues da rodada que ficaram analisadas.
    pub analysed: usize,
    /// O planejamento inicial da janela terminou agora.
    pub planned: bool,
}

/// Correção manual de uma aresta na UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManualEdit {
    /// Aresta manual ativa (vale mesmo que a análise não a veja).
    Add,
    /// Lápide: a aresta sai e a análise não a recria.
    Remove,
    /// Apaga a correção manual: volta a valer o que a análise disser.
    Reset,
}

/// Situação do bloqueante para a fila.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum BlockerState {
    /// PR mesclado na branch de integração, ou concluída ou fechada.
    Ready,
    /// No espelho, ainda não pronta.
    Pending,
    /// Fora do espelho do projeto (outro projeto, inexistente): bloqueia e
    /// vira aviso.
    Unknown,
}

/// A rodada de análise aberta de um projeto (veja o módulo).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnalysisRound {
    pub project_id: i64,
    /// Sessão de análise da rodada; `None` = nenhuma viva (falhou ou ainda
    /// não iniciou).
    pub session_id: Option<String>,
    pub env_id: String,
    /// Quando a janela abriu para o projeto (a rodada começou).
    pub opened_at: i64,
    /// Início da sessão atual.
    pub started_at: Option<i64>,
    /// Fim do planejamento inicial; `None` = em andamento.
    pub planned_at: Option<i64>,
    /// Issues enviadas à sessão e ainda sem resposta, com a impressão
    /// digital do envio.
    pub pending: BTreeMap<i64, String>,
    /// Último envio à sessão (início ou reanálise).
    pub sent_at: Option<i64>,
    /// Última falha da sessão (não iniciou ou terminou sem gravar).
    pub failed_at: Option<i64>,
}

/// Impressão digital de uma issue da fila: muda quando o título, a
/// atualização no rastreador (corpo, rótulos, comentários), a prioridade ou
/// os rótulos mudam. FNV-1a de 64 bits, estável entre versões.
pub fn issue_fingerprint(
    title: &str,
    updated_at: i64,
    priority: Option<&str>,
    labels: &[String],
) -> String {
    let mut sorted: Vec<&str> = labels.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let text = format!(
        "{title}\u{1f}{updated_at}\u{1f}{}\u{1f}{}",
        priority.unwrap_or("-"),
        sorted.join("\u{1e}")
    );
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn clip(s: &str) -> String {
    let t = s.trim();
    match t.char_indices().nth(DEP_REASON_MAX_CHARS) {
        Some((i, _)) => t[..i].to_string(),
        None => t.to_string(),
    }
}

const ROUND_COLS: &str = "project_id, session_id, env_id, opened_at, started_at, planned_at, \
                          pending, sent_at, failed_at";

fn row_to_round(r: &rusqlite::Row) -> rusqlite::Result<AnalysisRound> {
    let pending: String = r.get(6)?;
    Ok(AnalysisRound {
        project_id: r.get(0)?,
        session_id: r.get(1)?,
        env_id: r.get(2)?,
        opened_at: r.get(3)?,
        started_at: r.get(4)?,
        planned_at: r.get(5)?,
        pending: serde_json::from_str(&pending).unwrap_or_default(),
        sent_at: r.get(7)?,
        failed_at: r.get(8)?,
    })
}

impl Hub {
    /// Todas as arestas do projeto, inclusive as lápides manuais, por issue
    /// e bloqueante.
    pub fn autopilot_dependencies(&self, project_id: i64) -> Result<Vec<DependencyEdge>> {
        let mut stmt = self.conn.prepare(
            "SELECT number, blocker, reason, source, removed, updated_at
             FROM autopilot_dependency WHERE project_id = ?1 ORDER BY number, blocker",
        )?;
        let rows = stmt.query_map([project_id], |r| {
            Ok(DependencyEdge {
                number: r.get(0)?,
                blocker: r.get(1)?,
                reason: r.get(2)?,
                source: r.get(3)?,
                removed: r.get(4)?,
                updated_at: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Grava o resultado da sessão de análise do projeto: substitui as
    /// arestas de análise de cada issue informada (nunca as manuais), a
    /// ordem sugerida (se veio) e marca como analisadas as issues enviadas à
    /// sessão na rodada, com a impressão digital do envio. O planejamento
    /// inicial da janela termina aqui. Sem rodada aberta = `Conflict`.
    pub fn set_dependencies_from_analysis(
        &self,
        project_id: i64,
        set: &DependencySet,
        now: i64,
    ) -> Result<DependencySetResult> {
        if set.items.len() > DEP_MAX_ITEMS {
            return Err(LedgerError::Invalid(format!(
                "no máximo {DEP_MAX_ITEMS} issues por gravação"
            )));
        }
        let mut seen = BTreeSet::new();
        for i in &set.items {
            if i.number <= 0 {
                return Err(LedgerError::Invalid(format!(
                    "número de issue inválido: {}",
                    i.number
                )));
            }
            if !seen.insert(i.number) {
                return Err(LedgerError::Invalid(format!(
                    "issue #{} repetida em items",
                    i.number
                )));
            }
            if i.blocked_by.len() > DEP_MAX_BLOCKERS {
                return Err(LedgerError::Invalid(format!(
                    "issue #{}: no máximo {DEP_MAX_BLOCKERS} bloqueantes",
                    i.number
                )));
            }
            if let Some(b) = i.blocked_by.iter().find(|b| b.number <= 0) {
                return Err(LedgerError::Invalid(format!(
                    "issue #{}: bloqueante inválido {}",
                    i.number, b.number
                )));
            }
        }
        if set.order.iter().any(|n| *n <= 0) {
            return Err(LedgerError::Invalid(
                "order: só números de issue positivos".into(),
            ));
        }
        let Some(mut round) = self.autopilot_analysis(project_id)? else {
            return Err(LedgerError::Conflict(
                "nenhuma rodada de análise aberta neste projeto: a janela da política está \
                 fechada"
                    .into(),
            ));
        };
        let tx = self.tx()?;
        let mut out = DependencySetResult::default();
        for i in &set.items {
            self.conn.execute(
                "DELETE FROM autopilot_dependency
                 WHERE project_id = ?1 AND number = ?2 AND source = 'analysis'",
                params![project_id, i.number],
            )?;
            let (edges, kept) =
                self.insert_analysis_edges(project_id, i.number, &i.blocked_by, now)?;
            out.edges += edges;
            out.kept_manual += kept;
            out.items += 1;
        }
        if !set.order.is_empty() {
            self.conn.execute(
                "DELETE FROM autopilot_order WHERE project_id = ?1",
                [project_id],
            )?;
            let mut placed = BTreeSet::new();
            for n in &set.order {
                if placed.insert(*n) {
                    self.conn.execute(
                        "INSERT INTO autopilot_order (project_id, number, position)
                         VALUES (?1, ?2, ?3)",
                        params![project_id, n, placed.len() as i64],
                    )?;
                }
            }
        }
        for (n, fp) in &round.pending {
            self.conn.execute(
                "INSERT INTO autopilot_analysed (project_id, number, fingerprint, analysed_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (project_id, number) DO UPDATE SET
                   fingerprint = excluded.fingerprint, analysed_at = excluded.analysed_at",
                params![project_id, n, fp, now],
            )?;
            out.analysed += 1;
        }
        round.pending.clear();
        if round.planned_at.is_none() {
            round.planned_at = Some(now);
            out.planned = true;
        }
        self.save_autopilot_analysis(&round)?;
        tx.commit()?;
        Ok(out)
    }

    /// Grava as arestas de análise de `number` (sem apagar as anteriores:
    /// quem chama apaga). Uma issue não bloqueia a si mesma; repetida vale
    /// uma vez; a aresta corrigida à mão (ativa ou lápide) fica. Devolve as
    /// gravadas e as que ficaram por causa da manual.
    pub(crate) fn insert_analysis_edges(
        &self,
        project_id: i64,
        number: i64,
        blocked_by: &[BlockerInput],
        now: i64,
    ) -> Result<(usize, usize)> {
        let (mut edges, mut kept) = (0, 0);
        let mut blockers = BTreeSet::new();
        for b in blocked_by {
            if b.number == number || !blockers.insert(b.number) {
                continue;
            }
            let n = self.conn.execute(
                "INSERT INTO autopilot_dependency
                   (project_id, number, blocker, reason, source, removed, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 'analysis', 0, ?5)
                 ON CONFLICT (project_id, number, blocker) DO NOTHING",
                params![project_id, number, b.number, clip(&b.reason), now],
            )?;
            if n > 0 {
                edges += 1;
            } else {
                kept += 1;
            }
        }
        Ok((edges, kept))
    }

    /// Correção manual de uma aresta (veja [`ManualEdit`]).
    pub fn set_dependency_manual(
        &self,
        project_id: i64,
        number: i64,
        blocker: i64,
        edit: ManualEdit,
        reason: &str,
        now: i64,
    ) -> Result<()> {
        if number <= 0 || blocker <= 0 {
            return Err(LedgerError::Invalid(
                "number e blocker são números de issue".into(),
            ));
        }
        if number == blocker {
            return Err(LedgerError::Invalid(
                "uma issue não bloqueia a si mesma".into(),
            ));
        }
        match edit {
            ManualEdit::Reset => {
                self.conn.execute(
                    "DELETE FROM autopilot_dependency
                     WHERE project_id = ?1 AND number = ?2 AND blocker = ?3 AND source = 'manual'",
                    params![project_id, number, blocker],
                )?;
            }
            ManualEdit::Add | ManualEdit::Remove => {
                let removed = edit == ManualEdit::Remove;
                self.conn.execute(
                    "INSERT INTO autopilot_dependency
                       (project_id, number, blocker, reason, source, removed, updated_at)
                     VALUES (?1, ?2, ?3, ?4, 'manual', ?5, ?6)
                     ON CONFLICT (project_id, number, blocker) DO UPDATE SET
                       reason = CASE WHEN excluded.reason = '' THEN reason
                                     ELSE excluded.reason END,
                       source = 'manual', removed = excluded.removed,
                       updated_at = excluded.updated_at",
                    params![project_id, number, blocker, clip(reason), removed, now],
                )?;
            }
        }
        Ok(())
    }

    /// Ordem de execução sugerida pela análise: número → posição (1, 2...).
    pub fn autopilot_order(&self, project_id: i64) -> Result<BTreeMap<i64, i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT number, position FROM autopilot_order WHERE project_id = ?1")?;
        let rows = stmt.query_map([project_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Impressão digital de cada issue na última análise que a incluiu.
    pub fn autopilot_analysed(&self, project_id: i64) -> Result<BTreeMap<i64, String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT number, fingerprint FROM autopilot_analysed WHERE project_id = ?1")?;
        let rows = stmt.query_map([project_id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// A rodada de análise aberta do projeto.
    pub fn autopilot_analysis(&self, project_id: i64) -> Result<Option<AnalysisRound>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {ROUND_COLS} FROM autopilot_analysis WHERE project_id = ?1"),
                [project_id],
                row_to_round,
            )
            .optional()?)
    }

    /// Todas as rodadas abertas.
    pub fn autopilot_analyses(&self) -> Result<Vec<AnalysisRound>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {ROUND_COLS} FROM autopilot_analysis ORDER BY project_id"
        ))?;
        let rows = stmt.query_map([], row_to_round)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Grava (abre ou atualiza) a rodada do projeto.
    pub fn save_autopilot_analysis(&self, r: &AnalysisRound) -> Result<()> {
        let pending = serde_json::to_string(&r.pending).expect("BTreeMap serializa");
        self.conn.execute(
            &format!(
                "INSERT OR REPLACE INTO autopilot_analysis ({ROUND_COLS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
            ),
            params![
                r.project_id,
                r.session_id,
                r.env_id,
                r.opened_at,
                r.started_at,
                r.planned_at,
                pending,
                r.sent_at,
                r.failed_at
            ],
        )?;
        Ok(())
    }

    /// Fecha a rodada do projeto (fim da janela); `false` se não havia.
    pub fn close_autopilot_analysis(&self, project_id: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM autopilot_analysis WHERE project_id = ?1",
            [project_id],
        )? > 0)
    }

    /// Estado da sessão no hub; `None` = o ambiente ainda não a sincronizou.
    pub fn autopilot_session_state(&self, session_id: &str) -> Result<Option<SessionState>> {
        Ok(self
            .conn
            .query_row(
                "SELECT state FROM session WHERE id = ?1",
                [session_id],
                |r| crate::parse_col::<SessionState>(r, 0),
            )
            .optional()?)
    }

    /// Atualização no rastreador e rótulos de cada issue do espelho do
    /// projeto, para a impressão digital.
    pub fn autopilot_issue_meta(
        &self,
        project_id: i64,
    ) -> Result<BTreeMap<i64, (i64, Vec<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT gh_number, updated_at, labels FROM issue_cache WHERE project_id = ?1",
        )?;
        let rows = stmt.query_map([project_id], |r| {
            let labels: String = r.get(2)?;
            Ok((
                r.get::<_, i64>(0)?,
                (
                    r.get::<_, i64>(1)?,
                    serde_json::from_str::<Vec<String>>(&labels).unwrap_or_default(),
                ),
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Situação de cada bloqueante (veja [`BlockerState`]): pronto com PR
    /// mesclado na branch de integração `integration`, ou concluído ou
    /// fechado.
    pub fn autopilot_blocker_states(
        &self,
        project_id: i64,
        integration: &str,
        numbers: &BTreeSet<i64>,
    ) -> Result<BTreeMap<i64, BlockerState>> {
        let mut out = BTreeMap::new();
        for n in numbers {
            let (in_mirror, ready): (bool, bool) = self.conn.query_row(
                "SELECT
                   EXISTS (SELECT 1 FROM issue_cache WHERE project_id = ?1 AND gh_number = ?2)
                   OR EXISTS (SELECT 1 FROM work_item WHERE project_id = ?1 AND gh_number = ?2),
                   EXISTS (SELECT 1 FROM issue_cache
                           WHERE project_id = ?1 AND gh_number = ?2 AND state = 'closed')
                   OR EXISTS (SELECT 1 FROM work_item
                              WHERE project_id = ?1 AND gh_number = ?2 AND status = 'concluido')
                   OR EXISTS (SELECT 1 FROM pr_cache c JOIN work_item w ON w.id = c.work_item_id
                              WHERE c.project_id = ?1 AND w.gh_number = ?2
                                AND c.base = ?3 AND c.state = 'merged')",
                params![project_id, n, integration],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let st = if ready {
                BlockerState::Ready
            } else if in_mirror {
                BlockerState::Pending
            } else {
                BlockerState::Unknown
            };
            out.insert(*n, st);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GithubSnapshot, IssueRow, NewHubProject, PrRow};
    use ct_core::PrState;

    fn issue(n: i64, state: &str) -> IssueRow {
        IssueRow {
            gh_number: n,
            title: format!("ACME-{n}"),
            state: state.into(),
            url: String::new(),
            updated_at: 1,
            ..Default::default()
        }
    }

    fn hub() -> (Hub, i64) {
        let hub = Hub::open_in_memory().unwrap();
        let p = hub
            .add_project(&NewHubProject {
                owner: "acme".into(),
                repo: "acme-web".into(),
                name: "acme-web".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        (hub, p.project.id)
    }

    fn open(hub: &Hub, pid: i64, pending: &[(i64, &str)]) {
        hub.save_autopilot_analysis(&AnalysisRound {
            project_id: pid,
            session_id: Some("s-analise".into()),
            env_id: "env-auto".into(),
            opened_at: 10,
            started_at: Some(10),
            pending: pending.iter().map(|(n, f)| (*n, f.to_string())).collect(),
            sent_at: Some(10),
            ..Default::default()
        })
        .unwrap();
    }

    fn set(items: &[(i64, &[i64])]) -> DependencySet {
        DependencySet {
            items: items
                .iter()
                .map(|(n, bs)| DependencyInput {
                    number: *n,
                    blocked_by: bs
                        .iter()
                        .map(|b| BlockerInput {
                            number: *b,
                            reason: format!("precisa da #{b}"),
                        })
                        .collect(),
                })
                .collect(),
            order: vec![],
        }
    }

    fn active(hub: &Hub, pid: i64) -> Vec<(i64, i64, String)> {
        hub.autopilot_dependencies(pid)
            .unwrap()
            .into_iter()
            .filter(|e| !e.removed)
            .map(|e| (e.number, e.blocker, e.source))
            .collect()
    }

    #[test]
    fn impressao_digital_muda_com_o_que_importa() {
        let l = vec!["b".to_string(), "a".to_string()];
        let base = issue_fingerprint("Tela", 1, Some("p1"), &l);
        assert_eq!(base.len(), 16);
        // Ordem dos rótulos não importa.
        let l2 = vec!["a".to_string(), "b".to_string()];
        assert_eq!(issue_fingerprint("Tela", 1, Some("p1"), &l2), base);
        assert_ne!(issue_fingerprint("Tela", 2, Some("p1"), &l), base);
        assert_ne!(issue_fingerprint("Tela", 1, Some("p0"), &l), base);
        assert_ne!(issue_fingerprint("Tela", 1, Some("p1"), &l[..1]), base);
        assert_ne!(issue_fingerprint("Tela 2", 1, Some("p1"), &l), base);
    }

    #[test]
    fn sem_rodada_aberta_recusa() {
        let (hub, pid) = hub();
        let e = hub
            .set_dependencies_from_analysis(pid, &set(&[(12, &[11])]), 20)
            .unwrap_err();
        assert!(matches!(e, LedgerError::Conflict(_)), "{e}");
    }

    #[test]
    fn analise_grava_arestas_ordem_e_issues_analisadas() {
        let (hub, pid) = hub();
        open(&hub, pid, &[(12, "f12"), (13, "f13")]);
        let mut s = set(&[(12, &[11, 11, 12]), (13, &[])]);
        s.order = vec![11, 13, 12, 13];
        let r = hub.set_dependencies_from_analysis(pid, &s, 20).unwrap();
        assert_eq!(
            r,
            DependencySetResult {
                items: 2,
                edges: 1,
                kept_manual: 0,
                analysed: 2,
                planned: true
            }
        );
        assert_eq!(active(&hub, pid), vec![(12, 11, "analysis".into())]);
        assert_eq!(
            hub.autopilot_order(pid).unwrap(),
            BTreeMap::from([(11, 1), (13, 2), (12, 3)])
        );
        assert_eq!(
            hub.autopilot_analysed(pid).unwrap(),
            BTreeMap::from([(12, "f12".into()), (13, "f13".into())])
        );
        let round = hub.autopilot_analysis(pid).unwrap().unwrap();
        assert!(round.pending.is_empty());
        assert_eq!(round.planned_at, Some(20));
        // Segunda gravação: substitui as da issue, o planejamento já tinha
        // terminado, e a ordem vazia mantém a anterior.
        let r = hub
            .set_dependencies_from_analysis(pid, &set(&[(12, &[13])]), 30)
            .unwrap();
        assert!(!r.planned);
        assert_eq!(active(&hub, pid), vec![(12, 13, "analysis".into())]);
        assert_eq!(hub.autopilot_order(pid).unwrap().len(), 3);
        assert_eq!(
            hub.autopilot_analysis(pid).unwrap().unwrap().planned_at,
            Some(20)
        );
    }

    #[test]
    fn correcao_manual_prevalece_sobre_a_analise() {
        let (hub, pid) = hub();
        open(&hub, pid, &[]);
        hub.set_dependencies_from_analysis(pid, &set(&[(12, &[11]), (13, &[11])]), 20)
            .unwrap();
        // A pessoa remove 12 ← 11 e adiciona 13 ← 14.
        hub.set_dependency_manual(pid, 12, 11, ManualEdit::Remove, "", 21)
            .unwrap();
        hub.set_dependency_manual(pid, 13, 14, ManualEdit::Add, "migração", 21)
            .unwrap();
        assert_eq!(
            active(&hub, pid),
            vec![(13, 11, "analysis".into()), (13, 14, "manual".into())]
        );
        // A análise seguinte vê 12 ← 11 de novo e não vê 13 ← 14: as
        // manuais ficam como a pessoa deixou.
        let r = hub
            .set_dependencies_from_analysis(pid, &set(&[(12, &[11]), (13, &[])]), 30)
            .unwrap();
        assert_eq!(r.kept_manual, 1);
        assert_eq!(active(&hub, pid), vec![(13, 14, "manual".into())]);
        let lapide = hub
            .autopilot_dependencies(pid)
            .unwrap()
            .into_iter()
            .find(|e| e.number == 12)
            .unwrap();
        assert!(lapide.removed);
        assert_eq!(lapide.reason, "precisa da #11");
        // Restaurar apaga a manual: a próxima análise volta a valer.
        hub.set_dependency_manual(pid, 12, 11, ManualEdit::Reset, "", 31)
            .unwrap();
        hub.set_dependencies_from_analysis(pid, &set(&[(12, &[11])]), 32)
            .unwrap();
        assert!(active(&hub, pid).contains(&(12, 11, "analysis".into())));
        assert!(hub
            .set_dependency_manual(pid, 12, 12, ManualEdit::Add, "", 33)
            .is_err());
    }

    #[test]
    fn validacao_da_gravacao() {
        let (hub, pid) = hub();
        open(&hub, pid, &[]);
        for bad in [
            set(&[(0, &[])]),
            set(&[(12, &[]), (12, &[])]),
            set(&[(12, &[-1])]),
            DependencySet {
                order: vec![0],
                ..Default::default()
            },
        ] {
            assert!(matches!(
                hub.set_dependencies_from_analysis(pid, &bad, 20),
                Err(LedgerError::Invalid(_))
            ));
        }
    }

    #[test]
    fn rodada_abre_atualiza_e_fecha() {
        let (hub, pid) = hub();
        assert!(hub.autopilot_analysis(pid).unwrap().is_none());
        open(&hub, pid, &[(12, "f")]);
        let mut r = hub.autopilot_analysis(pid).unwrap().unwrap();
        assert_eq!(r.pending, BTreeMap::from([(12, "f".into())]));
        r.session_id = None;
        r.failed_at = Some(40);
        hub.save_autopilot_analysis(&r).unwrap();
        assert_eq!(hub.autopilot_analyses().unwrap(), vec![r]);
        assert!(hub.close_autopilot_analysis(pid).unwrap());
        assert!(!hub.close_autopilot_analysis(pid).unwrap());
    }

    #[test]
    fn situacao_dos_bloqueantes() {
        let (hub, pid) = hub();
        let wi = hub.ensure_work_item("acme", "acme-web", 13).unwrap();
        hub.apply_github(
            &GithubSnapshot {
                project_id: pid,
                issues: vec![issue(11, "open"), issue(12, "closed"), issue(13, "open")],
                prs: vec![PrRow {
                    number: 50,
                    work_item_id: wi,
                    head: "feat/13".into(),
                    base: "wip/autonoma".into(),
                    state: PrState::Merged,
                    url: String::new(),
                    updated_at: 1,
                    head_sha: None,
                }],
                ..Default::default()
            },
            "hub-id",
        )
        .unwrap();
        let st = hub
            .autopilot_blocker_states(pid, "wip/autonoma", &BTreeSet::from([11, 12, 13, 99]))
            .unwrap();
        assert_eq!(st[&11], BlockerState::Pending);
        assert_eq!(st[&12], BlockerState::Ready);
        assert_eq!(st[&99], BlockerState::Unknown);
        assert_eq!(st[&13], BlockerState::Ready);
        // Sem o status concluído, só o PR mesclado na branch de integração
        // conta: em outra base, não.
        hub.conn
            .execute(
                "UPDATE work_item SET status = 'em_revisao' WHERE id = ?1",
                [wi],
            )
            .unwrap();
        let only = |base: &str| {
            hub.autopilot_blocker_states(pid, base, &BTreeSet::from([13]))
                .unwrap()[&13]
        };
        assert_eq!(only("wip/autonoma"), BlockerState::Ready);
        assert_eq!(only("outra"), BlockerState::Pending);
    }
}
