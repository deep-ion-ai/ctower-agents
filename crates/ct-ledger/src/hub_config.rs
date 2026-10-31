//! Configuração vinda do hub (spec do hub como plano de controle §4; issue
//! #177): o contrato de `GET /sync/config` e o cache dela no `ledger.db`.
//!
//! O hub monta a resposta com [`crate::Hub::env_config`]; o ambiente grava
//! com [`Ledger::apply_hub_config`]. A tabela `project` vira **cache somente
//! leitura**: só o pull escreve nas linhas vindas do hub (`config_rev` não
//! nulo), e gravar o cache **nunca** põe fotografia de projeto no outbox. A
//! única mudança de projeto que sobe é o relato da aplicação
//! ([`Ledger::record_apply`]), que sempre leva `state` e `applied_rev`.

use crate::projects::{row_to_project, COLS, COLS_LEN};
use crate::sync::project_key;
use crate::{ConfigRules, Ledger, LocationState, Project, Result};
use ct_core::{now_ms, DocsConfig, ForgeConfig, TrackerConfig};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Projeto atribuído ao ambiente, como vem em `GET /sync/config`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigProject {
    pub owner: String,
    pub repo: String,
    pub name: String,
    pub base_branch: String,
    pub protected_branches: Vec<String>,
    /// Caminho pedido pelo hub; vazio = `<repos_root>/<repo>` do ambiente.
    pub repo_path: String,
    /// `config_rev` da última mudança do projeto ou da atribuição (o maior
    /// dos dois). O ambiente relata esse valor em `applied_rev`.
    pub rev: i64,
    /// Rastreador e forja (spec do Jira e GitLab §3.5). Ausentes num hub
    /// anterior à #347: GitHub.
    #[serde(default)]
    pub tracker: TrackerConfig,
    #[serde(default)]
    pub forge: ForgeConfig,
    /// Padrão do nome da branch; `None` = o padrão do rastreador.
    #[serde(default)]
    pub branch_pattern: Option<String>,
    /// Linhas de atribuição nas sessões; ausente = ligadas.
    #[serde(default = "attribution_default")]
    pub attribution: bool,
    /// Este ambiente é o de sync do projeto (§5.1): envia o espelho do
    /// rastreador ao hub.
    #[serde(default)]
    pub sync: bool,
    /// Regras do agente vigentes do projeto e dos épicos (issue #357, spec
    /// §10.2). Ausentes num hub anterior: nenhuma.
    #[serde(default, skip_serializing_if = "ConfigRules::is_empty")]
    pub rules: ConfigRules,
    /// Pasta de docs do projeto fora do repositório (#440). Ausente num hub
    /// anterior: desligada.
    #[serde(default)]
    pub docs: DocsConfig,
}

fn attribution_default() -> bool {
    true
}

/// Atribuição removida (ou projeto excluído) depois do `since` pedido.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigRemoved {
    pub owner: String,
    pub repo: String,
    pub rev: i64,
}

/// Resposta de `GET /sync/config?since=<rev>`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigResponse {
    pub config_rev: i64,
    /// Época da configuração do hub: muda no `cthubd restore`. O ambiente a
    /// devolve no pedido; época diferente = conjunto completo.
    #[serde(default)]
    pub config_epoch: i64,
    /// `false` = `since` igual ao `config_rev` do hub (na mesma época); sem
    /// `projects`.
    pub changed: bool,
    /// Conjunto **completo** atribuído ao ambiente (não só o delta).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<ConfigProject>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed: Option<Vec<ConfigRemoved>>,
}

/// Work item que o hub deu como `concluido`, em `GET /sync/done`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConfigDone {
    pub owner: String,
    pub repo: String,
    pub number: i64,
    /// Quando o hub o concluiu (ms).
    pub since: Option<i64>,
}

/// Resposta de `GET /sync/done`: o conjunto **completo** dos projetos
/// atribuídos ao ambiente.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct DoneResponse {
    pub items: Vec<ConfigDone>,
}

/// O que um pull mudou no cache (chaves `owner/repo`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ConfigApplied {
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub unassigned: Vec<String>,
}

impl ConfigApplied {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.updated.is_empty() && self.unassigned.is_empty()
    }
}

impl Ledger {
    pub(crate) fn meta(&self, key: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM ledger_meta WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    fn set_meta(&self, key: &str, value: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO ledger_meta (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// `config_rev` do último pull aplicado; `None` = nunca puxou.
    pub fn hub_config_rev(&self) -> Result<Option<i64>> {
        self.meta("config_rev")
    }

    /// Época do hub no último pull aplicado.
    pub fn hub_config_epoch(&self) -> Result<Option<i64>> {
        self.meta("config_epoch")
    }

    /// Quando o último pull respondeu (com ou sem mudança).
    pub fn hub_config_pulled_at(&self) -> Result<Option<i64>> {
        self.meta("config_pulled_at")
    }

    /// Substitui o cache dos work items `concluido` do hub (#298). Itens de
    /// projeto que o ambiente não tem ficam de fora.
    pub fn apply_hub_done(&self, done: &DoneResponse) -> Result<()> {
        self.atomic(|| {
            self.conn.execute("DELETE FROM hub_done", [])?;
            for d in &done.items {
                let Some(p) = self.project_by_key(&project_key(&d.owner, &d.repo))? else {
                    continue;
                };
                self.conn.execute(
                    "INSERT OR REPLACE INTO hub_done (project_id, gh_number, since)
                     VALUES (?1, ?2, ?3)",
                    params![p.id, d.number, d.since],
                )?;
            }
            Ok(())
        })
    }

    /// O hub deu o work item como `concluido`? `Some(since)` = sim, desde
    /// quando (ms) se conhecido.
    pub fn hub_done(&self, project_id: i64, gh_number: i64) -> Result<Option<Option<i64>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT since FROM hub_done WHERE project_id = ?1 AND gh_number = ?2",
                params![project_id, gh_number],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Grava a configuração do hub no cache, numa transação, **sem pôr nada
    /// no outbox**. Com `changed: false`, só registra a hora do pull.
    ///
    /// - cada projeto de `projects` é casado por `owner/repo` (sem
    ///   diferenciar maiúsculas) com qualquer linha, inclusive a legada ou a
    ///   desatribuída, que mantém o `id` local (e o histórico ligado a ela);
    /// - `resolve` dá o caminho do clone (`repo_path` vazio = padrão do
    ///   ambiente);
    /// - toda linha que não veio em `projects` ganha `unassigned_at` (inclusive
    ///   as legadas que o hub não atribuiu); nada é apagado;
    /// - o caminho efetivo é resolvido de novo em todo pull (mesmo com
    ///   `changed: false`): se ele mudou (ex.: `repos_root` novo), o relato
    ///   local é zerado e a atribuição volta a pendente, para ser reaplicada
    ///   e relatada com o caminho novo.
    pub fn apply_hub_config(
        &self,
        cfg: &ConfigResponse,
        resolve: impl Fn(&ConfigProject) -> String,
    ) -> Result<ConfigApplied> {
        let now = now_ms();
        self.atomic(|| {
            let mut out = ConfigApplied::default();
            self.set_meta("config_pulled_at", now)?;
            let Some(projects) = cfg.projects.as_ref().filter(|_| cfg.changed) else {
                self.relocate(&resolve, &mut out)?;
                return Ok(out);
            };
            let mut assigned: HashSet<String> = HashSet::new();
            for p in projects {
                let key = project_key(&p.owner, &p.repo);
                if !assigned.insert(key.clone()) {
                    continue;
                }
                let path = resolve(p);
                let protected =
                    serde_json::to_string(&p.protected_branches).expect("Vec<String> serializa");
                match self.project_by_key(&key)? {
                    Some(cur) => {
                        let name = self.free_name(&p.name, &key, Some(cur.id))?;
                        let same = cur.name == name
                            && cur.gh_owner == p.owner
                            && cur.gh_repo == p.repo
                            && cur.repo_path == path
                            && cur.base_branch == p.base_branch
                            && cur.protected_branches == p.protected_branches
                            && cur.config_rev == Some(p.rev)
                            && cur.tracker == p.tracker
                            && cur.forge == p.forge
                            && cur.branch_pattern == p.branch_pattern
                            && cur.attribution == p.attribution
                            && cur.sync_role == p.sync
                            && cur.docs == p.docs
                            && cur.unassigned_at.is_none()
                            && self.hub_repo_path(cur.id)?.as_deref() == Some(&p.repo_path);
                        if same {
                            continue;
                        }
                        self.conn.execute(
                            "UPDATE project SET name = ?2, gh_owner = ?3, gh_repo = ?4,
                                    repo_path = ?5, base_branch = ?6, protected_branches = ?7,
                                    config_rev = ?8, unassigned_at = NULL, hub_repo_path = ?9
                             WHERE id = ?1",
                            params![
                                cur.id,
                                name,
                                p.owner,
                                p.repo,
                                path,
                                p.base_branch,
                                protected,
                                p.rev,
                                p.repo_path
                            ],
                        )?;
                        self.set_provider(cur.id, p)?;
                        if cur.repo_path != path {
                            self.reset_report(cur.id)?;
                        }
                        // Virou o ambiente de sync (§5.3): manda o espelho
                        // inteiro, sem depender do que o anterior enviou.
                        if p.sync && !cur.sync_role {
                            self.enqueue_issue_mirror(cur.id)?;
                        }
                        out.updated.push(key);
                    }
                    None => {
                        let name = self.free_name(&p.name, &key, None)?;
                        self.conn.execute(
                            "INSERT INTO project (name, gh_owner, gh_repo, repo_path, base_branch,
                                                  protected_branches, created_at, config_rev,
                                                  hub_repo_path)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                            params![
                                name,
                                p.owner,
                                p.repo,
                                path,
                                p.base_branch,
                                protected,
                                now,
                                p.rev,
                                p.repo_path
                            ],
                        )?;
                        self.set_provider(self.conn.last_insert_rowid(), p)?;
                        out.added.push(key);
                    }
                }
            }
            // Regras do agente: o cache inteiro de cada projeto vem de novo
            // em todo pull com mudança (o conjunto é completo).
            for p in projects {
                if let Some(cur) = self.project_by_key(&project_key(&p.owner, &p.repo))? {
                    self.replace_agent_rules(cur.id, &p.rules)?;
                }
            }
            for p in self.list_projects()? {
                let key = project_key(&p.gh_owner, &p.gh_repo);
                if !assigned.contains(&key) {
                    self.replace_agent_rules(p.id, &ConfigRules::default())?;
                    self.conn.execute(
                        "UPDATE project SET unassigned_at = ?2 WHERE id = ?1",
                        params![p.id, now],
                    )?;
                    // Ninguém mais sincroniza o projeto: a última falha ficaria
                    // parada e a Sala de controle a mostraria como "sync atrasado".
                    self.conn
                        .execute("DELETE FROM sync_state WHERE project_id = ?1", [p.id])?;
                    out.unassigned.push(key);
                }
            }
            self.set_meta("config_rev", cfg.config_rev)?;
            self.set_meta("config_epoch", cfg.config_epoch)?;
            Ok(out)
        })
    }

    /// Rastreador, forja, padrão de branch, atribuição e papel de sync do
    /// projeto, como vieram do hub.
    fn set_provider(&self, id: i64, p: &ConfigProject) -> Result<()> {
        self.conn.execute(
            "UPDATE project SET tracker_kind = ?2, tracker_config = ?3, forge_kind = ?4,
                    forge_config = ?5, branch_pattern = ?6, attribution = ?7, sync_role = ?8,
                    docs_config = ?9
             WHERE id = ?1",
            params![
                id,
                p.tracker.kind().as_str(),
                p.tracker.config_json(),
                p.forge.kind().as_str(),
                p.forge.config_json(),
                p.branch_pattern,
                p.attribution,
                p.sync,
                p.docs.config_json()
            ],
        )?;
        Ok(())
    }

    fn hub_repo_path(&self, id: i64) -> Result<Option<String>> {
        Ok(self.conn.query_row(
            "SELECT hub_repo_path FROM project WHERE id = ?1",
            [id],
            |r| r.get(0),
        )?)
    }

    /// Zera o relato local: a atribuição volta a pendente e é reaplicada. A
    /// fotografia no outbox não muda (o hub mantém o relato anterior até o
    /// novo chegar).
    fn reset_report(&self, id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE project SET location_state = NULL, location_error = NULL,
                    applied_rev = NULL
             WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    /// Resolve de novo o caminho das atribuições ativas (o `repos_root` ou o
    /// `HOME` podem ter mudado sem mudança no hub).
    fn relocate(
        &self,
        resolve: &impl Fn(&ConfigProject) -> String,
        out: &mut ConfigApplied,
    ) -> Result<()> {
        let rows: Vec<(Project, String)> = {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT {COLS}, hub_repo_path FROM project
                 WHERE config_rev IS NOT NULL AND unassigned_at IS NULL
                   AND hub_repo_path IS NOT NULL
                 ORDER BY id"
            ))?;
            let rows = stmt.query_map([], |r| Ok((row_to_project(r)?, r.get(COLS_LEN)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (p, hub_path) in rows {
            let path = resolve(&ConfigProject {
                owner: p.gh_owner.clone(),
                repo: p.gh_repo.clone(),
                name: p.name.clone(),
                base_branch: p.base_branch.clone(),
                protected_branches: p.protected_branches.clone(),
                repo_path: hub_path,
                rev: p.config_rev.unwrap_or_default(),
                tracker: p.tracker.clone(),
                forge: p.forge.clone(),
                branch_pattern: p.branch_pattern.clone(),
                attribution: p.attribution,
                sync: p.sync_role,
                rules: Default::default(),
                docs: p.docs.clone(),
            });
            if path != p.repo_path {
                self.conn.execute(
                    "UPDATE project SET repo_path = ?2 WHERE id = ?1",
                    params![p.id, path],
                )?;
                self.reset_report(p.id)?;
                out.updated.push(project_key(&p.gh_owner, &p.gh_repo));
            }
        }
        Ok(())
    }

    /// Linha do projeto por `owner/repo` (a de menor `id`, se houver
    /// duplicadas legadas).
    fn project_by_key(&self, key: &str) -> Result<Option<Project>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {COLS} FROM project
                     WHERE lower(gh_owner || '/' || gh_repo) = ?1 ORDER BY id LIMIT 1"
                ),
                [key],
                row_to_project,
            )
            .optional()?)
    }

    /// `name` é único no `ledger.db`; no hub, não. Em caso de colisão com
    /// outra linha, usa `name (owner/repo)`.
    fn free_name(&self, name: &str, key: &str, id: Option<i64>) -> Result<String> {
        let taken = |n: &str| -> Result<bool> {
            Ok(self.conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM project WHERE name = ?1 AND id IS NOT ?2)",
                params![n, id],
                |r| r.get(0),
            )?)
        };
        if !taken(name)? {
            return Ok(name.to_string());
        }
        Ok(format!("{name} ({key})"))
    }

    /// Projetos do hub atribuídos cuja atribuição falta aplicar: nunca
    /// aplicada, aplicada numa revisão anterior ou com erro (tentada de novo
    /// a cada pull).
    pub fn pending_applies(&self) -> Result<Vec<Project>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM project
             WHERE config_rev IS NOT NULL AND unassigned_at IS NULL
               AND (applied_rev IS NULL OR applied_rev < config_rev
                    OR location_state IS NOT 'ok')
             ORDER BY id"
        ))?;
        let rows = stmt.query_map([], row_to_project)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Grava o resultado da aplicação da atribuição `rev` e, se ele mudou,
    /// põe no outbox o relato ao hub (`project_location` com `state`,
    /// `error` e `applied_rev`). Devolve se enfileirou.
    pub fn record_apply(
        &self,
        project_id: i64,
        rev: i64,
        state: LocationState,
        error: Option<&str>,
    ) -> Result<bool> {
        self.atomic(|| {
            let changed = self.conn.execute(
                "UPDATE project SET location_state = ?2, location_error = ?3, applied_rev = ?4
                 WHERE id = ?1 AND config_rev IS NOT NULL
                   AND (location_state IS NOT ?2 OR location_error IS NOT ?3
                        OR applied_rev IS NOT ?4)",
                params![project_id, state.as_str(), error, rev],
            )?;
            if changed > 0 {
                self.enqueue_project(project_id)?;
            }
            Ok(changed > 0)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::ledger_with_project;
    use crate::{LedgerError, ProjectPatch, SyncChange};

    fn cp(repo: &str, rev: i64) -> ConfigProject {
        ConfigProject {
            docs: Default::default(),
            owner: "deep-ion-ai".into(),
            repo: repo.into(),
            name: repo.into(),
            base_branch: "develop".into(),
            protected_branches: vec!["develop".into(), "main".into()],
            repo_path: String::new(),
            rev,
            tracker: TrackerConfig::Github,
            forge: ForgeConfig::Github,
            branch_pattern: None,
            attribution: true,
            sync: false,
            rules: Default::default(),
        }
    }

    fn resp(rev: i64, projects: Vec<ConfigProject>) -> ConfigResponse {
        ConfigResponse {
            config_rev: rev,
            config_epoch: 7,
            changed: true,
            projects: Some(projects),
            removed: Some(vec![]),
        }
    }

    fn path(p: &ConfigProject) -> String {
        if p.repo_path.is_empty() {
            format!("/r/{}", p.repo)
        } else {
            p.repo_path.clone()
        }
    }

    fn projetos_no_outbox(l: &Ledger) -> Vec<crate::ProjectChange> {
        l.outbox_pending(1000)
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.change {
                SyncChange::Project(p) => Some(p),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn cache_casa_legado_e_desatribui_o_resto_sem_outbox() {
        let (l, legado) = ledger_with_project();
        l.outbox_ack(i64::MAX).unwrap();
        assert_eq!(l.hub_config_rev().unwrap(), None);

        let mut demo = cp("Demo", 5);
        demo.repo_path = "/x/demo".into();
        let r = l
            .apply_hub_config(&resp(6, vec![demo, cp("novo", 6)]), path)
            .unwrap();
        assert_eq!(r.added, vec!["deep-ion-ai/novo"]);
        assert_eq!(r.updated, vec!["deep-ion-ai/demo"]);
        assert!(r.unassigned.is_empty());
        // O legado mantém o id local e vira cache do hub.
        let p = l.get_project(legado.id).unwrap();
        assert_eq!((p.config_rev, p.repo_path.as_str()), (Some(5), "/x/demo"));
        assert_eq!(p.gh_repo, "Demo");
        assert_eq!(l.hub_config_rev().unwrap(), Some(6));
        assert!(l.hub_config_pulled_at().unwrap().is_some());
        // Gravar o cache não gera fotografia de projeto no outbox.
        assert!(projetos_no_outbox(&l).is_empty());
        assert_eq!(l.pending_applies().unwrap().len(), 2);

        l.record_sync_error(legado.id, "falha de rede").unwrap();

        // Remoção pela ausência em `projects`: sai da lista, a linha fica.
        let r = l
            .apply_hub_config(&resp(7, vec![cp("novo", 6)]), path)
            .unwrap();
        assert_eq!(r.unassigned, vec!["deep-ion-ai/demo"]);
        assert!(r.updated.is_empty() && r.added.is_empty());
        let nomes: Vec<_> = l
            .list_projects()
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(nomes, vec!["novo"]);
        let fora = l.list_unassigned_projects().unwrap();
        assert_eq!(fora.len(), 1);
        assert!(fora[0].unassigned_at.is_some());
        assert!(!l.project_active(legado.id).unwrap());
        assert!(
            l.sync_state(legado.id).unwrap().is_none(),
            "sem sync_state de projeto fora do hub"
        );
        assert!(projetos_no_outbox(&l).is_empty());

        // `changed: false`: só a hora do pull.
        let sem = ConfigResponse {
            config_rev: 7,
            config_epoch: 7,
            changed: false,
            projects: None,
            removed: None,
        };
        assert!(l.apply_hub_config(&sem, path).unwrap().is_empty());
        assert_eq!(l.list_projects().unwrap().len(), 1);

        // Reatribuído: volta com o mesmo id.
        l.apply_hub_config(&resp(8, vec![cp("demo", 8), cp("novo", 6)]), path)
            .unwrap();
        let p = l.get_project(legado.id).unwrap();
        assert_eq!((p.unassigned_at, p.config_rev), (None, Some(8)));
    }

    #[test]
    fn legado_nao_atribuido_vira_projeto_fora_do_hub() {
        let (l, legado) = ledger_with_project();
        l.apply_hub_config(&resp(1, vec![]), path).unwrap();
        assert!(l.list_projects().unwrap().is_empty());
        let fora = l.list_unassigned_projects().unwrap();
        assert_eq!((fora[0].id, fora[0].config_rev), (legado.id, None));
    }

    #[test]
    fn relato_leva_applied_rev_e_toda_fotografia_repete_o_relato() {
        let (l, legado) = ledger_with_project();
        l.apply_hub_config(&resp(3, vec![cp("demo", 3)]), path)
            .unwrap();
        l.outbox_ack(i64::MAX).unwrap();

        // Erro: relatado com `applied_rev`.
        assert!(l
            .record_apply(legado.id, 3, LocationState::Error, Some("clone falhou"))
            .unwrap());
        let ps = projetos_no_outbox(&l);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].state, Some(LocationState::Error));
        assert_eq!(ps[0].error.as_deref(), Some("clone falhou"));
        assert_eq!(ps[0].applied_rev, Some(3));
        // O mesmo erro de novo não reenfileira; e continua pendente (tenta de novo).
        assert!(!l
            .record_apply(legado.id, 3, LocationState::Error, Some("clone falhou"))
            .unwrap());
        assert_eq!(l.pending_applies().unwrap().len(), 1);

        assert!(l
            .record_apply(legado.id, 3, LocationState::Ok, None)
            .unwrap());
        assert!(l.pending_applies().unwrap().is_empty());
        l.outbox_ack(i64::MAX).unwrap();

        // Pulls repetidos, iguais: nada muda, nada no outbox.
        for _ in 0..3 {
            let r = l
                .apply_hub_config(&resp(3, vec![cp("demo", 3)]), path)
                .unwrap();
            assert!(r.is_empty(), "{r:?}");
        }
        assert!(projetos_no_outbox(&l).is_empty());

        // Qualquer outra fotografia do projeto (resync) leva o relato.
        l.enqueue_all().unwrap();
        let ps = projetos_no_outbox(&l);
        assert_eq!(ps.len(), 1);
        assert_eq!(
            (ps[0].state, ps[0].applied_rev),
            (Some(LocationState::Ok), Some(3))
        );

        // Mudança no hub: pendente até aplicar o `rev` novo.
        let mut novo = cp("demo", 4);
        novo.base_branch = "main".into();
        l.apply_hub_config(&resp(4, vec![novo]), path).unwrap();
        let p = l.get_project(legado.id).unwrap();
        assert_eq!(p.base_branch, "main");
        assert_eq!(l.pending_applies().unwrap().len(), 1);
    }

    #[test]
    fn cache_do_hub_e_somente_leitura() {
        let (l, legado) = ledger_with_project();
        l.apply_hub_config(&resp(1, vec![cp("demo", 1)]), path)
            .unwrap();
        let patch = ProjectPatch {
            base_branch: Some("x".into()),
            ..Default::default()
        };
        let err = l.update_project(legado.id, &patch).unwrap_err();
        assert!(matches!(err, LedgerError::Conflict(_)), "{err}");
        assert_eq!(l.get_project(legado.id).unwrap().base_branch, "develop");
    }

    #[test]
    fn nome_repetido_no_hub_ganha_sufixo() {
        let (l, _) = ledger_with_project();
        let mut outro = cp("outro", 2);
        outro.name = "demo".into();
        l.apply_hub_config(&resp(2, vec![cp("demo", 1), outro]), path)
            .unwrap();
        let mut nomes: Vec<_> = l
            .list_projects()
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        nomes.sort();
        assert_eq!(nomes, vec!["demo", "demo (deep-ion-ai/outro)"]);
    }

    #[test]
    fn resposta_sem_mudanca_serializa_so_o_rev() {
        let v = serde_json::to_value(ConfigResponse {
            config_rev: 4,
            config_epoch: 9,
            changed: false,
            projects: None,
            removed: None,
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "config_rev": 4, "config_epoch": 9, "changed": false })
        );
    }

    /// `repos_root` mudou no ambiente (sem mudança no hub): o caminho é
    /// resolvido de novo e a atribuição volta a pendente, para reclonar ou
    /// validar e relatar o caminho novo.
    #[test]
    fn caminho_efetivo_novo_volta_a_pendente() {
        let (l, legado) = ledger_with_project();
        l.apply_hub_config(&resp(3, vec![cp("demo", 3)]), path)
            .unwrap();
        assert_eq!(l.get_project(legado.id).unwrap().repo_path, "/r/demo");
        l.record_apply(legado.id, 3, LocationState::Ok, None)
            .unwrap();
        l.outbox_ack(i64::MAX).unwrap();
        assert!(l.pending_applies().unwrap().is_empty());

        let outro_root = |p: &ConfigProject| format!("/novo/{}", p.repo);
        // Pull sem mudança no hub, com outra raiz.
        let sem = ConfigResponse {
            config_rev: 3,
            config_epoch: 7,
            changed: false,
            projects: None,
            removed: None,
        };
        let r = l.apply_hub_config(&sem, outro_root).unwrap();
        assert_eq!(r.updated, vec!["deep-ion-ai/demo"]);
        let p = l.get_project(legado.id).unwrap();
        assert_eq!(p.repo_path, "/novo/demo");
        assert_eq!((p.location_state, p.applied_rev), (None, None));
        let pend = l.pending_applies().unwrap();
        assert_eq!(pend.len(), 1);
        // Nada no outbox até a reaplicação relatar.
        assert!(projetos_no_outbox(&l).is_empty());
        assert!(l
            .record_apply(legado.id, 3, LocationState::Ok, None)
            .unwrap());
        let ps = projetos_no_outbox(&l);
        assert_eq!(
            (ps[0].repo_path.as_str(), ps[0].applied_rev),
            ("/novo/demo", Some(3))
        );
        // Estável: o mesmo pull de novo não muda nada.
        assert!(l.apply_hub_config(&sem, outro_root).unwrap().is_empty());

        // Também com `changed: true` (conjunto completo).
        let r = l
            .apply_hub_config(&resp(3, vec![cp("demo", 3)]), path)
            .unwrap();
        assert_eq!(r.updated, vec!["deep-ion-ai/demo"]);
        assert_eq!(l.get_project(legado.id).unwrap().applied_rev, None);
        assert_eq!(l.hub_config_epoch().unwrap(), Some(7));
    }

    /// Projeto Jira + GitLab (#347): rastreador, forja, padrão de branch,
    /// atribuição e papel de sync vêm do pull e mudam com ele; um hub
    /// anterior (sem os campos) dá GitHub.
    #[test]
    fn pull_traz_rastreador_e_forja() {
        let l = Ledger::open_in_memory().unwrap();
        let jira = TrackerConfig::Jira(ct_core::JiraConfig {
            url: "https://jira.example.com".into(),
            project_key: "PROJ".into(),
            ..Default::default()
        });
        let forge = ForgeConfig::Gitlab(ct_core::GitlabConfig {
            host: "gitlab.example.com".into(),
            path: "grupo/projeto/app".into(),
            ..Default::default()
        });
        let mut p = ConfigProject {
            owner: "grupo/projeto".into(),
            tracker: jira.clone(),
            forge: forge.clone(),
            branch_pattern: Some("{key}".into()),
            attribution: false,
            sync: true,
            rules: Default::default(),
            ..cp("app", 1)
        };
        l.apply_hub_config(&resp(1, vec![p.clone()]), path).unwrap();
        let got = &l.list_projects().unwrap()[0];
        assert_eq!((&got.tracker, &got.forge), (&jira, &forge));
        assert_eq!(got.branch_pattern.as_deref(), Some("{key}"));
        assert_eq!(got.effective_branch_pattern(), "{key}");
        assert!(!got.attribution && got.sync_role);
        assert_eq!(got.item_ref(5450), "PROJ-5450");
        assert_eq!(
            (got.gh_owner.as_str(), got.gh_repo.as_str()),
            ("grupo/projeto", "app")
        );

        // Mudança só do papel de sync: a linha é atualizada.
        p.sync = false;
        let r = l.apply_hub_config(&resp(2, vec![p.clone()]), path).unwrap();
        assert_eq!(r.updated, vec!["grupo/projeto/app"]);
        assert!(!l.list_projects().unwrap()[0].sync_role);

        // Voltar a ser o ambiente de sync (#349) enfileira o espelho inteiro
        // do cache, que fora do papel não ia ao hub.
        let issue = crate::IssueRow {
            gh_number: 5450,
            title: "Portar o plugin".into(),
            state: "open".into(),
            labels: vec![],
            parent_number: None,
            has_sub_issues: false,
            url: "https://jira.example.com/browse/PROJ-5450".into(),
            updated_at: 1,
            ..Default::default()
        };
        let snap = crate::GithubSnapshot {
            project_id: got.id,
            issues: vec![issue.clone()],
            ..Default::default()
        };
        assert_eq!(l.apply_github(&snap).unwrap().mirrored, 0);
        l.outbox_ack(i64::MAX).unwrap();
        p.sync = true;
        l.apply_hub_config(&resp(3, vec![p.clone()]), path).unwrap();
        let out = l.outbox_pending(10).unwrap();
        assert!(
            out.iter().any(|e| matches!(&e.change,
                crate::SyncChange::IssueMirror(m) if m.issue == issue)),
            "{out:?}"
        );

        // Hub anterior à #347: sem os campos novos, o projeto é GitHub.
        let v = serde_json::json!({
            "owner": "deep-ion-ai", "repo": "demo", "name": "demo", "base_branch": "develop",
            "protected_branches": [], "repo_path": "", "rev": 3
        });
        let antigo: ConfigProject = serde_json::from_value(v).unwrap();
        assert_eq!(antigo.tracker, TrackerConfig::Github);
        assert_eq!(antigo.forge, ForgeConfig::Github);
        assert!(antigo.attribution && !antigo.sync && antigo.branch_pattern.is_none());
        l.apply_hub_config(&resp(4, vec![antigo]), path).unwrap();
        let demo = &l.list_projects().unwrap()[0];
        assert_eq!(demo.tracker, TrackerConfig::Github);
        assert_eq!(demo.effective_branch_pattern(), "{prefix}/{n}-{slug}");
        assert_eq!(demo.item_ref(7), "deep-ion-ai/demo#7");
    }

    /// Docs do projeto (#440): vêm no pull e mudam com ele; um hub anterior
    /// (sem o campo) dá docs desligados.
    #[test]
    fn pull_com_e_sem_docs() {
        let l = Ledger::open_in_memory().unwrap();
        let docs = DocsConfig {
            enabled: true,
            project_dirs: vec!["analises".into()],
            epic_dirs: vec!["prompts".into()],
        };
        let p = ConfigProject {
            docs: docs.clone(),
            ..cp("demo", 1)
        };
        l.apply_hub_config(&resp(1, vec![p.clone()]), path).unwrap();
        assert_eq!(l.list_projects().unwrap()[0].docs, docs);

        // Só o modelo mudou: a linha é atualizada.
        let p2 = ConfigProject {
            docs: DocsConfig {
                epic_dirs: vec!["prompts".into(), "evidencias".into()],
                ..docs.clone()
            },
            rev: 2,
            ..p.clone()
        };
        let r = l
            .apply_hub_config(&resp(2, vec![p2.clone()]), path)
            .unwrap();
        assert_eq!(r.updated, vec!["deep-ion-ai/demo"]);
        assert_eq!(l.list_projects().unwrap()[0].docs, p2.docs);

        // Hub anterior à #440: sem `docs`, desligado.
        let v = serde_json::json!({
            "owner": "deep-ion-ai", "repo": "demo", "name": "demo", "base_branch": "develop",
            "protected_branches": [], "repo_path": "", "rev": 3
        });
        let antigo: ConfigProject = serde_json::from_value(v).unwrap();
        assert_eq!(antigo.docs, DocsConfig::default());
        l.apply_hub_config(&resp(3, vec![antigo]), path).unwrap();
        assert!(!l.list_projects().unwrap()[0].docs.enabled);
        // E o JSON de hoje leva o campo.
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["docs"]["project_dirs"][0], "analises");
    }
}
