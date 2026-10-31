//! Sync do projeto Jira (+ GitLab) no ambiente (spec do Jira e GitLab §5.2,
//! §6, issue #349): roda o comando de credencial uma vez por ciclo, confere
//! a prontidão do jira-cli, busca no Jira sem o lock do ledger e aplica com
//! `apply_github`, como o `GithubSync` faz com o GitHub.
//!
//! A listagem (`issue list --raw`) só traz chave e `updated` confiáveis: a
//! issue nova ou com `updated` diferente do cache é lida com `issue view
//! --raw`, que tem a categoria do status e os campos do épico. O recorte é
//! incremental (`updated >= -<m>m`), com reconciliação diária e no primeiro
//! ciclo (abertas e fechadas nos últimos 30 dias). A marca da última
//! reconciliação fica no `gh_etag` (`jira:full:<owner>/<repo>`), com a
//! configuração que a gerou: mudar `type_map`, os campos do épico ou o da
//! sprint força uma reconciliação que relê todas as issues; subir o
//! `MIRROR_VERSION` (o que a leitura extrai mudou, como as sprints) também,
//! uma vez. Mudar o `jql` força a reconciliação sem reler o que não mudou.
//!
//! O JQL define o conjunto espelhado: na reconciliação de um projeto com
//! JQL, a listagem do JQL puro diz o que ficou de fora, e isso sai do
//! espelho (no ambiente e, pelo `issue_mirror` com `removed`, no hub), menos
//! as issues com work item e os ancestrais delas, que ficam em
//! `jira:pinned:<owner>/<repo>` e são lidas pela chave em todo ciclo. Sem
//! poda com a listagem falhando ou vazia, ou, com o JQL igual ao do último
//! ciclo, tirando mais da metade do espelho.
//!
//! As issues lidas são gravadas em lotes (`batch_size`, 200), cada um numa
//! transação (`apply_tracker_batch`), com o progresso no log e na prontidão
//! (`progress`, que sobe ao hub no `tracker_status`). O cursor incremental
//! (`last_ok_at`) e a marca da reconciliação só avançam no `apply_github` do
//! fim: um ciclo interrompido deixa os lotes gravados, e o próximo recomeça
//! do mesmo recorte, pulando o que já está no cache com o mesmo `updated`.
//! A falha de leitura de uma issue não derruba o ciclo: a chave vai para
//! `jira:retry:<owner>/<repo>` no `gh_etag` (lida de novo no próximo ciclo)
//! e o ciclo termina com aviso no `last_error`. Senha recusada, jira-cli
//! sumido ou `MAX_FAILS_IN_A_ROW` falhas seguidas (Jira fora do ar) derrubam
//! o ciclo.
//!
//! No mesmo ciclo, o GitLab (#350): o comando de credencial do GitLab, a
//! prontidão do glab (`glab version`, `glab api version`) e os MRs da branch
//! de cada work item em andamento (`glab mr list -s <branch>`), que entram
//! no `GithubSnapshot.prs` como os PRs do GitHub: o MR mesclado ou fechado dá
//! o mesmo gatilho de status, o mesmo evento e a mesma conclusão do PR.
//!
//! No ambiente de sync do projeto (`sync_role`), as issues e os MRs que
//! mudaram sobem como `issue_mirror` e `pr_mirror` (dentro do
//! `apply_github`) e o fim de cada ciclo, com ou sem erro, como
//! `tracker_status`.

use crate::github::ProjectSync;
use crate::gitlab::{project_path, GitlabRemote};
use crate::{Result, Sessions, SessionsError};
use ct_core::{
    now_ms, GitlabConfig, GitlabReadiness, JiraConfig, JiraReadiness, PrState, TrackerKind,
    TrackerProgress, TrackerReadiness, WorkItemStatus,
};
use ct_gitlab::{mr_row, Glab};
use ct_jira::{
    issue_row, list_entry, run_credential, server_version, JiraCli, JiraError, ListEvent,
    ListQuery, Secret, CREDENTIAL_DEADLINE, DEADLINE, LIST_DEADLINE, LIST_RETRY_WAITS, PAGE_SIZE,
};
use ct_ledger::{project_key, GithubSnapshot, IssueRow, Ledger, PrRow, Project};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Intervalo entre reconciliações completas.
const FULL_EVERY_MS: i64 = 24 * 3_600_000;
/// Folga do recorte incremental, em minutos, além do tempo desde o último
/// sucesso.
const INCREMENTAL_SLACK_MIN: i64 = 5;
/// `issue view` em paralelo (o primeiro ciclo lê todas as issues).
const VIEW_WORKERS: usize = 4;
/// Issues lidas por lote: cada lote vai ao ledger (e ao hub) quando
/// termina, para o primeiro ciclo não deixar o projeto vazio até o fim.
pub const BATCH_SIZE: usize = 200;
/// Intervalo mínimo entre os `tracker_status` de progresso no outbox.
const PROGRESS_EVERY_MS: i64 = 30_000;
/// `issue view` falhando em seguida que derrubam o ciclo (Jira fora do ar).
const MAX_FAILS_IN_A_ROW: usize = 10;
/// Chaves citadas no aviso das issues não lidas.
const FAILED_SHOWN: usize = 5;
/// Páginas da listagem entre um log de progresso e outro.
const LIST_LOG_EVERY_PAGES: usize = 5;

/// Binários e prazos do jira-cli e do glab.
#[derive(Debug, Clone)]
pub struct TrackerSyncConfig {
    /// `jira_bin` do `config.toml` (padrão `"jira"`).
    pub jira_bin: String,
    /// `glab_bin` do `config.toml` (padrão `"glab"`).
    pub glab_bin: String,
    /// Prazo de cada chamada ao jira-cli fora a listagem (`issue view`,
    /// `me`, `serverinfo`); `jira_view_timeout_secs` do `config.toml`.
    pub view_deadline: Duration,
    /// Prazo de cada página do `issue list`; `jira_list_timeout_secs` do
    /// `config.toml`.
    pub list_deadline: Duration,
    /// Esperas antes de pedir de novo uma página da listagem que falhou por
    /// prazo ou erro transitório (uma tentativa a mais por item).
    pub list_retry_waits: Vec<Duration>,
    /// Prazo de cada chamada ao glab.
    pub glab_deadline: Duration,
    /// Prazo do comando de credencial.
    pub credential_deadline: Duration,
    /// Issues por lote gravado (`BATCH_SIZE`).
    pub batch_size: usize,
}

impl TrackerSyncConfig {
    pub fn new(jira_bin: &str) -> Self {
        TrackerSyncConfig {
            jira_bin: jira_bin.to_string(),
            glab_bin: "glab".to_string(),
            view_deadline: DEADLINE,
            list_deadline: LIST_DEADLINE,
            list_retry_waits: LIST_RETRY_WAITS.to_vec(),
            glab_deadline: ct_gitlab::DEADLINE,
            credential_deadline: CREDENTIAL_DEADLINE,
            batch_size: BATCH_SIZE,
        }
    }

    /// Prazos do jira-cli: `view` para o `issue view` e as conferências,
    /// `list` para cada página do `issue list`.
    pub fn with_jira_deadlines(mut self, view: Duration, list: Duration) -> Self {
        self.view_deadline = view;
        self.list_deadline = list;
        self
    }

    pub fn with_glab_bin(mut self, glab_bin: &str) -> Self {
        self.glab_bin = glab_bin.to_string();
        self
    }

    /// O glab com os mesmos prazos, para a limpeza das branches remotas.
    pub fn gitlab(&self) -> GitlabRemote {
        GitlabRemote {
            glab_bin: self.glab_bin.clone(),
            cli_deadline: self.glab_deadline,
            credential_deadline: self.credential_deadline,
        }
    }
}

/// Chave da marca da última reconciliação no `gh_etag`.
pub fn full_mark_key(owner: &str, repo: &str) -> String {
    format!("jira:full:{}", project_key(owner, repo))
}

/// Chave, no `gh_etag`, das issues que o último ciclo concluído não
/// conseguiu ler (separadas por espaço): o ciclo seguinte lê de novo.
pub fn retry_key(owner: &str, repo: &str) -> String {
    format!("jira:retry:{}", project_key(owner, repo))
}

/// Versão do que a leitura extrai de cada issue. Subir o número faz a
/// marca da última reconciliação deixar de bater e o próximo ciclo reler o
/// projeto uma vez (como mudar a configuração): 2 = com as sprints; 3 = com
/// os metadados (`meta`).
pub const MIRROR_VERSION: i64 = 3;

/// Chave, no `gh_etag`, das issues fora do JQL que ficam no espelho por
/// ter work item (separadas por espaço): cada ciclo as lê pela chave, como
/// as de `retry_key`, porque a listagem com o JQL não as traz.
pub fn pinned_key(owner: &str, repo: &str) -> String {
    format!("jira:pinned:{}", project_key(owner, repo))
}

/// Fração do espelho acima da qual a poda, com o JQL igual ao do último
/// ciclo, é tida como erro (listagem incompleta) e não acontece.
const PRUNE_MAX_SHARE: f64 = 0.5;

/// O que muda o resultado da leitura de cada issue: com outro valor, a
/// reconciliação relê todas as issues listadas. O JQL fica de fora: ele
/// muda o conjunto espelhado, não o que se lê de cada issue.
///
/// Os campos do projeto (`extra_fields`, ID e rótulo) só entram quando há
/// algum: a marca do projeto sem eles fica igual à de antes.
fn read_fingerprint(c: &JiraConfig) -> serde_json::Value {
    let mut v = json!({
        "mirror": MIRROR_VERSION,
        "project_key": c.project_key,
        "type_map": c.type_map,
        "epic_link_field": c.epic_link_field,
        "epic_name_field": c.epic_name_field,
        "sprint_field": c.sprint_field.trim(),
        "url": c.url,
    });
    if !c.extra_fields.is_empty() {
        v["extra_fields"] = c
            .extra_fields
            .iter()
            .map(|f| json!([f.id.trim(), f.label.trim()]))
            .collect();
    }
    v
}

/// Chaves da marca que dizem o conjunto espelhado (o filtro), não o que se
/// lê de cada issue.
const FILTER_KEYS: [&str; 3] = ["jql", "active_sprint_only", "max_issues"];

/// O filtro do espelho: o JQL efetivo, o modo e o limite.
fn filter_fingerprint(c: &JiraConfig) -> [serde_json::Value; 3] {
    [
        c.effective_jql().into(),
        c.active_sprint_only.into(),
        c.issue_limit().into(),
    ]
}

/// A configuração da marca da reconciliação: a da leitura mais o filtro.
/// Com outro valor, o ciclo é uma reconciliação.
fn fingerprint(c: &JiraConfig) -> String {
    let mut v = read_fingerprint(c);
    for (k, f) in FILTER_KEYS.iter().zip(filter_fingerprint(c)) {
        v[*k] = f;
    }
    v.to_string()
}

/// O que a busca precisa do ledger, lido de uma vez (lock curto).
struct TrackerInput {
    cached: HashMap<i64, IssueRow>,
    /// `(quando, configuração)` da última reconciliação.
    mark: Option<(i64, String)>,
    last_ok_at: Option<i64>,
    /// Chaves que o último ciclo concluído não leu (`retry_key`).
    retry: Vec<String>,
    /// Chaves fora do JQL mantidas por ter work item (`pinned_key`).
    pinned: Vec<String>,
    /// Números das issues com work item (qualquer status) neste ambiente.
    work_items: HashSet<i64>,
    /// `(work item, branch)` cujos MRs o ciclo lê: com branch registrada,
    /// não concluído e com o último MR do espelho não mesclado.
    watched: Vec<(i64, String)>,
}

impl TrackerInput {
    fn load(l: &Ledger, project: &Project) -> ct_ledger::Result<Self> {
        let mark = l
            .etag(&full_mark_key(&project.gh_owner, &project.gh_repo))?
            .and_then(|m| {
                let (at, fp) = m.split_once(' ')?;
                Some((at.parse().ok()?, fp.to_string()))
            });
        Ok(TrackerInput {
            cached: l
                .list_issues(project.id)?
                .into_iter()
                .map(|i| (i.gh_number, i))
                .collect(),
            mark,
            last_ok_at: l.sync_state(project.id)?.and_then(|s| s.last_ok_at),
            retry: l
                .etag(&retry_key(&project.gh_owner, &project.gh_repo))?
                .map(|r| r.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
            pinned: l
                .etag(&pinned_key(&project.gh_owner, &project.gh_repo))?
                .map(|r| r.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
            work_items: l
                .list_work_items(project.id)?
                .iter()
                .map(|w| w.gh_number)
                .collect(),
            watched: watched_branches(l, project.id)?,
        })
    }
}

/// Work items cujos MRs o ciclo lê (spec §7.2), como os PRs do `GithubSync`:
/// com branch registrada e não concluídos, sem o que já tem o último MR
/// mesclado.
fn watched_branches(l: &Ledger, project_id: i64) -> ct_ledger::Result<Vec<(i64, String)>> {
    let mut out = Vec::new();
    for w in l.list_work_items(project_id)? {
        let Some(branch) = w.branch.filter(|b| !b.trim().is_empty()) else {
            continue;
        };
        if w.status == WorkItemStatus::Concluido {
            continue;
        }
        let last = l.prs_for_work_item(w.id)?.into_iter().next();
        if last.is_some_and(|p| p.state == PrState::Merged) {
            continue;
        }
        out.push((w.id, branch));
    }
    Ok(out)
}

/// Recorte de um ciclo.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Plan {
    /// Filtro do `-q` (o jira-cli acrescenta o `project = ...`).
    jql: String,
    /// Relê todas as issues listadas (a configuração mudou).
    reread_all: bool,
    /// Nova marca da reconciliação, gravada só com o sucesso do ciclo.
    mark: Option<String>,
    /// Reconciliação com JQL ou limite: confere o conjunto do filtro e poda
    /// o que saiu.
    prune: bool,
    /// O filtro (JQL, modo sprint ativa ou limite) mudou desde a última
    /// reconciliação (a poda grande é esperada).
    filter_changed: bool,
}

fn plan(cfg: &JiraConfig, input: &TrackerInput, now: i64) -> Plan {
    let fp = fingerprint(cfg);
    let filter = cfg.effective_jql();
    let wrap = |cond: &str| match filter.as_str() {
        "" => cond.to_string(),
        q => format!("({q}) AND {cond}"),
    };
    // A marca de antes, em JSON; a de antes das sprints não tem `mirror` e
    // a ilegível conta como outra configuração.
    let old: Option<serde_json::Value> = input
        .mark
        .as_ref()
        .and_then(|(_, f)| serde_json::from_str(f).ok());
    let reread_all = input.mark.is_some()
        && old
            .as_ref()
            .map(|o| {
                let mut o = o.clone();
                if let Some(m) = o.as_object_mut() {
                    for k in FILTER_KEYS {
                        m.remove(k);
                    }
                }
                o
            })
            .as_ref()
            != Some(&read_fingerprint(cfg));
    // A marca anterior ao modo e ao limite não tem essas chaves: conta como
    // filtro mudado (a primeira poda pelo limite é esperada).
    let filter_changed = input.mark.is_some()
        && old.as_ref().map(|o| {
            FILTER_KEYS
                .iter()
                .map(|k| o.get(*k).cloned())
                .collect::<Vec<_>>()
        }) != Some(filter_fingerprint(cfg).into_iter().map(Some).collect());
    let full = match (&input.mark, input.last_ok_at) {
        (Some((at, f)), Some(_)) => *f != fp || now - at >= FULL_EVERY_MS,
        _ => true,
    };
    if full {
        // Abertas e fechadas nos últimos 30 dias, como no GitHub. A
        // categoria vai pelo id (3 = Done): o nome aparece traduzido.
        return Plan {
            jql: wrap("(statusCategory != 3 OR updated >= -30d)"),
            reread_all,
            mark: Some(format!("{now} {fp}")),
            prune: !filter.is_empty() || cfg.issue_limit().is_some(),
            filter_changed,
        };
    }
    let since = input.last_ok_at.unwrap_or(now);
    // Minutos desde o último sucesso (arredondado para cima) mais a folga;
    // o prazo relativo é calculado pelo Jira, sem depender do fuso.
    let minutes = ((now - since).max(0) + 59_999) / 60_000 + INCREMENTAL_SLACK_MIN;
    Plan {
        jql: wrap(&format!("updated >= -{minutes}m")),
        reread_all: false,
        mark: None,
        prune: false,
        filter_changed: false,
    }
}

/// Erro de um ciclo: a mensagem vai ao `sync_state` (nunca com a senha).
type CycleError = String;

/// Credenciais do ciclo: a senha do Jira e o token do GitLab (`None` sem
/// forja GitLab).
type Tokens = (Secret, Option<Secret>);

pub struct JiraGitlabSync {
    sessions: Sessions,
    cfg: TrackerSyncConfig,
    /// Uma sincronização por vez (laço periódico × `sync.now`).
    running: Mutex<()>,
    /// Prontidão do último ciclo de cada projeto (`env.info.trackers`).
    readiness: Mutex<BTreeMap<i64, TrackerReadiness>>,
}

impl JiraGitlabSync {
    pub fn new(sessions: Sessions, cfg: TrackerSyncConfig) -> Self {
        JiraGitlabSync {
            sessions,
            cfg,
            running: Mutex::new(()),
            readiness: Mutex::new(BTreeMap::new()),
        }
    }

    /// O projeto é deste laço (rastreador Jira).
    pub fn handles(&self, project_id: i64) -> bool {
        self.sessions
            .ledger()
            .get_project(project_id)
            .is_ok_and(|p| p.tracker.kind() == TrackerKind::Jira)
    }

    /// Prontidão de cada projeto Jira atribuído, do último ciclo (sem rodar
    /// nada agora).
    pub fn readiness(&self) -> Vec<TrackerReadiness> {
        self.readiness
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    /// Todos os projetos Jira atribuídos. A falha de um não impede os
    /// outros: fica em `sync_state.last_error` e no resultado.
    pub fn sync_all(&self) -> Result<Vec<ProjectSync>> {
        let _one = self.running.lock().unwrap_or_else(|e| e.into_inner());
        let ids: Vec<i64> = self
            .sessions
            .ledger()
            .list_projects()?
            .iter()
            .filter(|p| p.tracker.kind() == TrackerKind::Jira)
            .map(|p| p.id)
            .collect();
        self.readiness
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|id, _| ids.contains(id));
        ids.into_iter().map(|id| self.sync_one(id)).collect()
    }

    /// Descrição crua da issue `number` (`issue view --raw`), para o "Ver
    /// issue" (#434): `{ title, description, url }`, com a `description` como
    /// o Jira a manda (wiki markup ou ADF; o hub converte). Nada é gravado.
    /// Síncrono: chame fora do runtime.
    pub fn issue_description(&self, project_id: i64, number: i64) -> Result<serde_json::Value> {
        let project = self.sessions.ledger().get_project(project_id)?;
        let Some(jira) = project.tracker.jira().cloned() else {
            return Err(SessionsError::Invalid("projeto sem rastreador Jira".into()));
        };
        let key = ct_core::item_key(&project.tracker_ref(), number);
        let cli = JiraCli::new(&self.cfg.jira_bin, &jira.config_file)
            .with_deadline(self.cfg.view_deadline);
        let fail = |e: String| SessionsError::Invalid(format!("Jira, {key}: {e}"));
        let token = run_credential("Jira", &jira.credential_cmd, self.cfg.credential_deadline)
            .map_err(|e| fail(e.to_string()))?;
        let v = cli.view(&token, &key).map_err(|e| fail(e.to_string()))?;
        Ok(json!({
            "title": v.pointer("/fields/summary").cloned().unwrap_or_default(),
            "description": v.pointer("/fields/description").cloned().unwrap_or_default(),
            "url": ct_core::item_url(&project.tracker_ref(), number),
        }))
    }

    pub fn sync_project(&self, project_id: i64) -> Result<ProjectSync> {
        let _one = self.running.lock().unwrap_or_else(|e| e.into_inner());
        self.sync_one(project_id)
    }

    fn sync_one(&self, project_id: i64) -> Result<ProjectSync> {
        let failed = |error: &str| ProjectSync {
            project_id,
            ok: false,
            error: Some(error.to_string()),
            issues: 0,
            prs: 0,
            status_changes: 0,
        };
        let project = {
            let l = self.sessions.ledger();
            if !l.project_active(project_id)? {
                return Ok(failed("projeto não atribuído a este ambiente"));
            }
            l.get_project(project_id)?
        };
        let Some(jira) = project.tracker.jira().cloned() else {
            return Ok(failed("projeto sem rastreador Jira"));
        };
        let now = now_ms();
        let cli = JiraCli::new(&self.cfg.jira_bin, &jira.config_file)
            .with_deadline(self.cfg.view_deadline)
            .with_list_deadline(self.cfg.list_deadline)
            .with_list_retry_waits(self.cfg.list_retry_waits.clone());

        let gitlab = project.forge.gitlab().cloned();
        let (readiness, tokens) = self.check(&project, &jira, &cli, gitlab.as_ref(), now);
        let slug = format!("{}/{}", project.gh_owner, project.gh_repo);
        // Issues gravadas nos lotes do ciclo.
        let mut batched = 0;
        type Read = (GithubSnapshot, Vec<(String, String)>, Option<Pruned>);
        let fetched: std::result::Result<Read, CycleError> = match tokens {
            Err(e) => Err(e),
            Ok((token, gl_token)) => {
                // Lock curto para ler; o jira-cli e o glab rodam sem ele, e
                // cada lote toma o lock só para gravar.
                let input = TrackerInput::load(&self.sessions.ledger(), &project)?;
                let batch_size = self.cfg.batch_size.max(1);
                let mut last_sent = None;
                let mut on_batch = |rows: Vec<IssueRow>, done: usize, total: usize| {
                    if !rows.is_empty() {
                        let a = self
                            .sessions
                            .ledger()
                            .apply_tracker_batch(&GithubSnapshot {
                                project_id,
                                issues: rows,
                                prs: Vec::new(),
                                etags: Vec::new(),
                                synced_at: now,
                                ..GithubSnapshot::default()
                            })
                            .map_err(|e| e.to_string())?;
                        batched += a.issues;
                    }
                    if done > 0 && total > batch_size {
                        tracing::info!("jira: {slug} {done} de {total} issues lidas");
                    }
                    let progress = TrackerProgress {
                        done: done as i64,
                        total: total as i64,
                        started_at: now,
                    };
                    self.progress(project_id, &readiness, progress, &mut last_sent)
                };
                let read = fetch(
                    &cli,
                    &token,
                    &jira,
                    &project,
                    &input,
                    now,
                    batch_size,
                    &mut on_batch,
                );
                read.and_then(|f| {
                    if f.total > 0 {
                        tracing::info!(
                            "jira: {slug} {} de {} issues lidas em {} s{}",
                            f.total - f.failed.len(),
                            f.total,
                            (now_ms() - now) / 1000,
                            match f.failed.len() {
                                0 => String::new(),
                                n => format!(" ({n} com falha)"),
                            }
                        );
                    }
                    let mut snap = GithubSnapshot {
                        project_id,
                        issues: Vec::new(),
                        prs: Vec::new(),
                        etags: f.etags,
                        synced_at: now,
                        ..GithubSnapshot::default()
                    };
                    if let (Some(gl), Some(t)) = (&gitlab, &gl_token) {
                        snap.prs = fetch_mrs(&self.gitlab_cli(gl), t, &project, gl, &input)?;
                    }
                    Ok((snap, f.failed, f.pruned))
                })
            }
        };
        let result = match fetched {
            Ok((snap, failed, pruned)) => {
                let applied = self.sessions.ledger().apply_github(&snap)?;
                if let Some(p) = pruned.filter(|p| !p.removed.is_empty() || !p.kept.is_empty()) {
                    let n = self
                        .sessions
                        .ledger()
                        .remove_tracker_issues(project_id, &p.removed)?;
                    tracing::info!(
                        "jira: {slug} {n} issue(s) fora do JQL removidas do espelho \
                         ({} mantidas por ter work item)",
                        p.kept.len()
                    );
                }
                for (wi, status) in &applied.status_changes {
                    self.sessions.notify(
                        "work_item_status",
                        json!({ "work_item_id": wi, "status": status }),
                    );
                }
                // O ciclo conta como concluído (o cursor avança), com aviso
                // das issues não lidas, que o próximo ciclo lê de novo.
                let warning = failed_warning(&failed);
                if let Some(w) = &warning {
                    tracing::warn!("jira: {slug}: {w}");
                    self.sessions.ledger().record_sync_warning(project_id, w)?;
                }
                ProjectSync {
                    project_id,
                    ok: true,
                    error: warning,
                    issues: batched + applied.issues,
                    prs: applied.prs,
                    status_changes: applied.status_changes.len(),
                }
            }
            Err(msg) => {
                self.sessions.ledger().record_sync_error(project_id, &msg)?;
                failed(&msg)
            }
        };

        let last_ok_at = {
            let l = self.sessions.ledger();
            // Só no ambiente de sync (o ledger confere `sync_role`).
            l.enqueue_tracker_status(project_id, &readiness)?;
            l.sync_state(project_id)?.and_then(|s| s.last_ok_at)
        };
        self.readiness
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(project_id, readiness);
        self.sessions.notify(
            "sync_status",
            json!({ "project_id": project_id, "ok": result.ok,
                    "last_ok_at": last_ok_at, "error": result.error }),
        );
        Ok(result)
    }

    /// Progresso da leitura: vai já na prontidão local (`env.info`) e, no
    /// máximo a cada `PROGRESS_EVERY_MS`, no `tracker_status` do outbox (só
    /// no ambiente de sync; a linha pendente é substituída pela nova).
    fn progress(
        &self,
        project_id: i64,
        readiness: &TrackerReadiness,
        progress: TrackerProgress,
        last_sent: &mut Option<i64>,
    ) -> std::result::Result<(), CycleError> {
        let r = TrackerReadiness {
            progress: Some(progress),
            ..readiness.clone()
        };
        let t = now_ms();
        if last_sent.is_none_or(|s| t - s >= PROGRESS_EVERY_MS) {
            self.sessions
                .ledger()
                .enqueue_tracker_status(project_id, &r)
                .map_err(|e| e.to_string())?;
            *last_sent = Some(t);
        }
        self.readiness
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(project_id, r);
        Ok(())
    }

    fn gitlab_cli(&self, gl: &GitlabConfig) -> Glab {
        self.cfg.gitlab().cli(gl)
    }

    /// Prontidão do jira-cli e do glab (§8.4) e as credenciais do ciclo
    /// (senha do Jira; token do GitLab, se a forja for GitLab). Cada lado
    /// para no primeiro item que impede a leitura, e os dois são conferidos
    /// sempre, para a prontidão mostrar tudo; o ciclo só segue com os dois
    /// prontos. `jira me` falhando só fica anotado (no jira-cli 1.7.0 ele não
    /// fala com o servidor).
    fn check(
        &self,
        project: &Project,
        jira: &JiraConfig,
        cli: &JiraCli,
        gitlab: Option<&GitlabConfig>,
        now: i64,
    ) -> (TrackerReadiness, std::result::Result<Tokens, CycleError>) {
        let mut r = TrackerReadiness {
            owner: project.gh_owner.clone(),
            repo: project.gh_repo.clone(),
            checked_at: now,
            ..TrackerReadiness::default()
        };
        let token = self.check_jira(&mut r.jira, jira, cli);
        if let Err(e) = &token {
            r.jira.error.get_or_insert_with(|| e.clone());
        }
        let gl_token = match gitlab {
            None => Ok(None),
            Some(gl) => {
                let t = self.check_gitlab(&mut r.gitlab, gl);
                if let Err(e) = &t {
                    r.gitlab.error.get_or_insert_with(|| e.clone());
                }
                t.map(Some)
            }
        };
        let tokens = token.and_then(|t| gl_token.map(|g| (t, g)));
        (r, tokens)
    }

    /// Prontidão do glab: versão, comando de credencial e `glab api
    /// version` com o token (confere host e token numa chamada leve).
    fn check_gitlab(
        &self,
        g: &mut GitlabReadiness,
        gl: &GitlabConfig,
    ) -> std::result::Result<Secret, CycleError> {
        let remote = self.cfg.gitlab();
        let cli = remote.cli(gl);
        g.cli = Some(cli.version().map_err(|e| e.to_string())?);
        if gl.host.trim().is_empty() {
            return Err("host do GitLab ausente no cadastro do projeto".into());
        }
        let token = remote.token(gl)?;
        g.credential_ok = true;
        cli.api_version(&token).map_err(|e| e.to_string())?;
        g.api_ok = true;
        Ok(token)
    }

    fn check_jira(
        &self,
        j: &mut JiraReadiness,
        jira: &JiraConfig,
        cli: &JiraCli,
    ) -> std::result::Result<Secret, CycleError> {
        j.cli = Some(cli.version().map_err(|e| e.to_string())?);
        j.config_present = cli.config_path().is_file();
        if !j.config_present {
            let path = match jira.config_file.trim() {
                "" => ct_jira::DEFAULT_CONFIG.to_string(),
                f => f.to_string(),
            };
            return Err(JiraError::ConfigMissing(path).to_string());
        }
        let server = cli.config_server();
        j.server_matches = server.as_deref().map(normalize_url) == Some(normalize_url(&jira.url));
        if !j.server_matches {
            return Err(format!(
                "o server da configuração do jira-cli ({}) não é a URL do cadastro ({}): \
                 rode \"jira init\" no ambiente com a URL do cadastro",
                server.as_deref().unwrap_or("ausente"),
                jira.url
            ));
        }
        // Catálogo do mesmo servidor do cadastro; sem a seção, lista vazia.
        j.custom_fields = cli.config_custom_fields();
        let token = run_credential("Jira", &jira.credential_cmd, self.cfg.credential_deadline)
            .map_err(|e| e.to_string())?;
        j.credential_ok = true;
        match cli.me(&token) {
            Ok(_) => j.me_ok = true,
            Err(e) => j.error = Some(format!("jira me: {e}")),
        }
        let info = cli.server_info(&token).map_err(|e| e.to_string())?;
        j.server_version = server_version(&info);
        Ok(token)
    }
}

/// `https://Jira.X/` e `https://jira.x` são o mesmo servidor.
fn normalize_url(u: &str) -> String {
    u.trim().trim_end_matches('/').to_lowercase()
}

/// O que a leitura do Jira deixa para o fim do ciclo (os lotes já foram
/// gravados): as ETags (marca da reconciliação e chaves a ler de novo) e as
/// leituras que falharam (`(chave, erro)`).
struct Fetched {
    etags: Vec<(String, String)>,
    failed: Vec<(String, String)>,
    /// Issues a ler no ciclo.
    total: usize,
    /// Poda da reconciliação (`None` = não houve).
    pruned: Option<Pruned>,
}

/// Issues fora do JQL: as que saem do espelho e as que ficam por ter work
/// item (lidas pela chave nos ciclos seguintes, `pinned_key`).
#[derive(Debug, Default, PartialEq, Eq)]
struct Pruned {
    removed: Vec<i64>,
    kept: Vec<i64>,
}

/// Recebe cada lote lido, com o progresso `(lidas, total)`; um lote vazio
/// com `0` lidas marca o início. Durante a listagem, vem vazio com
/// `(chaves listadas, 0)`.
type OnBatch<'a> =
    dyn FnMut(Vec<IssueRow>, usize, usize) -> std::result::Result<(), CycleError> + 'a;

/// Busca o projeto no Jira: lista o recorte do ciclo e lê cada issue nova ou
/// que mudou desde o cache, em lotes de `batch_size` entregues ao
/// `on_batch` (que grava). A marca da reconciliação e as chaves a ler de
/// novo só vão no `Fetched`, gravado no fim do ciclo.
#[allow(clippy::too_many_arguments)]
fn fetch(
    cli: &JiraCli,
    token: &Secret,
    jira: &JiraConfig,
    project: &Project,
    input: &TrackerInput,
    now: i64,
    batch_size: usize,
    on_batch: &mut OnBatch<'_>,
) -> std::result::Result<Fetched, CycleError> {
    let plan = plan(jira, input, now);
    let slug = format!("{}/{}", project.gh_owner, project.gh_repo);
    let mut progress_err = None;
    let listed = list_logged(
        cli,
        token,
        &ListQuery {
            project: jira.project_key.clone(),
            jql: plan.jql.clone(),
            limit: jira.issue_limit(),
        },
        &slug,
        &mut |n| {
            if progress_err.is_none() {
                progress_err = on_batch(Vec::new(), n, 0).err();
            }
        },
    )
    .map_err(|e| e.to_string())?;
    if let Some(e) = progress_err {
        return Err(e);
    }
    let mut seen = HashSet::new();
    let mut keys: Vec<String> = listed
        .iter()
        .filter_map(|v| list_entry(v, jira))
        .filter(|e| seen.insert(e.number))
        .filter(|e| {
            plan.reread_all
                || e.updated_at.is_none()
                || input.cached.get(&e.number).map(|c| c.updated_at) != e.updated_at
        })
        .map(|e| e.key)
        .collect();
    // No incremental, as que o último ciclo não leu (o recorte por
    // `updated` não as traria de novo). Na reconciliação, a listagem já
    // traz as que existem (fora do cache ou com outro `updated`), e a chave
    // que sumiu do Jira sai da lista.
    if plan.mark.is_none() {
        for k in &input.retry {
            if jira.number_from_key(k).is_some_and(|n| seen.insert(n)) {
                keys.push(k.clone());
            }
        }
    }
    // As de fora do JQL mantidas por ter work item: a listagem nunca as
    // traz, então são lidas pela chave em todo ciclo.
    for k in &input.pinned {
        if jira.number_from_key(k).is_some_and(|n| seen.insert(n)) {
            keys.push(k.clone());
        }
    }

    let total = keys.len();
    if total > 0 {
        on_batch(Vec::new(), 0, total)?;
    }
    let in_a_row = AtomicUsize::new(0);
    let mut failed = Vec::new();
    let mut done = 0;
    let mut applied = HashMap::new();
    let mut parents = HashSet::new();
    for chunk in keys.chunks(batch_size.max(1)) {
        let read = view_all(cli, token, chunk, &in_a_row);
        done += read.ok.len() + read.failed.len();
        for (key, e) in read.failed {
            tracing::warn!(
                "jira: {}/{} {key} não lida: {e}",
                project.gh_owner,
                project.gh_repo
            );
            failed.push((key, e.to_string()));
        }
        let rows = batch_rows(read.ok, jira, input, &mut applied, &mut parents);
        // O que o lote leu fica gravado mesmo se o ciclo cair agora.
        on_batch(rows, done, total)?;
        if let Some(e) = read.fatal {
            return Err(e);
        }
    }

    // Poda: só na reconciliação com JQL ou limite, com todas as leituras do ciclo
    // feitas e com algo já espelhado antes do ciclo (no primeiro, tudo o que
    // foi lido veio do próprio JQL).
    let pruned = if plan.prune && !input.cached.is_empty() {
        match list_logged(
            cli,
            token,
            &ListQuery {
                project: jira.project_key.clone(),
                jql: jira.effective_jql(),
                limit: jira.issue_limit(),
            },
            &slug,
            &mut |_| {},
        ) {
            Err(e) => {
                tracing::warn!("jira: {slug} listagem do JQL falhou, sem poda: {e}");
                None
            }
            Ok(listed) => {
                let inside: HashSet<i64> = listed
                    .iter()
                    .filter_map(|v| list_entry(v, jira))
                    .map(|e| e.number)
                    .collect();
                prune_plan(&slug, &inside, input, &applied, plan.filter_changed)
            }
        }
    } else {
        None
    };

    let mut etags: Vec<(String, String)> = plan
        .mark
        .map(|m| (full_mark_key(&project.gh_owner, &project.gh_repo), m))
        .into_iter()
        .collect();
    if let Some(p) = &pruned {
        let kept: Vec<String> = p
            .kept
            .iter()
            .map(|n| format!("{}-{n}", jira.project_key))
            .collect();
        etags.push((
            pinned_key(&project.gh_owner, &project.gh_repo),
            kept.join(" "),
        ));
    }
    if !failed.is_empty() || !input.retry.is_empty() {
        let retry: Vec<&str> = failed.iter().map(|(k, _)| k.as_str()).collect();
        etags.push((
            retry_key(&project.gh_owner, &project.gh_repo),
            retry.join(" "),
        ));
    }
    Ok(Fetched {
        etags,
        failed,
        total,
        pruned,
    })
}

/// `issue list` com o log do ciclo: cada nova tentativa de página e, a cada
/// `LIST_LOG_EVERY_PAGES` páginas, as chaves listadas até ali (também
/// entregues ao `on_listed`, a cada página). As novas tentativas ficam no
/// [`JiraCli::list_with`]; a reconciliação, o incremental e a listagem da
/// poda passam todos por aqui.
fn list_logged(
    cli: &JiraCli,
    token: &Secret,
    q: &ListQuery,
    slug: &str,
    on_listed: &mut dyn FnMut(usize),
) -> std::result::Result<Vec<serde_json::Value>, JiraError> {
    cli.list_with(token, q, &mut |e| match e {
        ListEvent::Page { offset, listed } => {
            if (offset / PAGE_SIZE + 1).is_multiple_of(LIST_LOG_EVERY_PAGES) {
                tracing::info!("jira: {slug} listando… {listed} chaves");
            }
            on_listed(listed);
        }
        ListEvent::Retry {
            offset,
            attempt,
            of,
            error,
            ..
        } => tracing::warn!(
            "jira: {slug} página {offset} da listagem falhou ({error}), tentando de novo \
             ({attempt}/{of})"
        ),
    })
}

/// Quais issues do espelho saem por estar fora do JQL (`inside` = o que a
/// listagem com o JQL trouxe). Ficam as com work item e os ancestrais delas
/// (o épico de uma sub-issue com work item). `None` = sem poda: listagem
/// vazia (provável erro) ou, com o filtro (JQL, modo e limite) igual ao do
/// último ciclo, poda de
/// mais de `PRUNE_MAX_SHARE` do espelho.
fn prune_plan(
    slug: &str,
    inside: &HashSet<i64>,
    input: &TrackerInput,
    applied: &HashMap<i64, IssueRow>,
    filter_changed: bool,
) -> Option<Pruned> {
    if inside.is_empty() {
        tracing::warn!("jira: {slug} a listagem do JQL veio vazia, sem poda");
        return None;
    }
    let row = |n: i64| applied.get(&n).or_else(|| input.cached.get(&n));
    let mut mirror: Vec<i64> = input
        .cached
        .keys()
        .chain(applied.keys())
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    mirror.sort();
    // Com work item, e cada ancestral (com limite, contra ciclo no dado).
    let mut keep = HashSet::new();
    for &n in &input.work_items {
        let mut cur = Some(n);
        let mut depth = 0;
        while let Some(c) = cur.filter(|_| depth < 16) {
            if !keep.insert(c) {
                break;
            }
            cur = row(c).and_then(|r| r.parent_number);
            depth += 1;
        }
    }
    let (kept, removed): (Vec<i64>, Vec<i64>) = mirror
        .iter()
        .copied()
        .filter(|n| !inside.contains(n))
        .partition(|n| keep.contains(n));
    if !filter_changed && removed.len() as f64 > mirror.len() as f64 * PRUNE_MAX_SHARE {
        tracing::warn!(
            "jira: {slug} a poda tiraria {} de {} issue(s) do espelho com o filtro igual ao \
             do último ciclo; provável listagem incompleta, sem poda",
            removed.len(),
            mirror.len()
        );
        return None;
    }
    Some(Pruned { removed, kept })
}

/// Linhas de um lote. Pai que ganhou filha (sub-task nova numa Story, que
/// não muda o `updated` dela) vira épico, como a issue com sub-issues no
/// GitHub; como a filha e o pai podem vir em lotes diferentes, o ciclo
/// guarda os pais já vistos (`parents`) e o que já gravou (`applied`).
fn batch_rows(
    views: Vec<serde_json::Value>,
    jira: &JiraConfig,
    input: &TrackerInput,
    applied: &mut HashMap<i64, IssueRow>,
    parents: &mut HashSet<i64>,
) -> Vec<IssueRow> {
    let mut rows: BTreeMap<i64, IssueRow> = BTreeMap::new();
    for v in views {
        if let Some(r) = issue_row(&v, jira) {
            rows.insert(r.gh_number, r);
        }
    }
    parents.extend(rows.values().filter_map(|r| r.parent_number));
    for r in rows.values_mut() {
        if parents.contains(&r.gh_number) {
            r.has_sub_issues = true;
        }
    }
    let outside: Vec<i64> = rows
        .values()
        .filter_map(|r| r.parent_number)
        .filter(|p| !rows.contains_key(p))
        .collect();
    for p in outside {
        let known = applied.get(&p).or_else(|| input.cached.get(&p));
        if let Some(c) = known.filter(|c| !c.has_sub_issues) {
            let mut c = c.clone();
            c.has_sub_issues = true;
            rows.insert(p, c);
        }
    }
    let rows: Vec<IssueRow> = rows.into_values().collect();
    applied.extend(rows.iter().map(|r| (r.gh_number, r.clone())));
    rows
}

/// Aviso do ciclo concluído com issues não lidas (vai no `last_error`).
fn failed_warning(failed: &[(String, String)]) -> Option<String> {
    let (_, first) = failed.first()?;
    let mut keys: Vec<&str> = failed
        .iter()
        .take(FAILED_SHOWN)
        .map(|(k, _)| k.as_str())
        .collect();
    if failed.len() > FAILED_SHOWN {
        keys.push("…");
    }
    Some(format!(
        "{} issue(s) não lidas, a ler de novo no próximo ciclo: {} ({first})",
        failed.len(),
        keys.join(", ")
    ))
}

/// MRs da branch de cada work item observado (§7.2): `glab mr list -s
/// <branch>`, só os da própria branch (o filtro do GitLab é exato, e a
/// conferência fica por garantia). A primeira falha derruba o ciclo.
fn fetch_mrs(
    cli: &Glab,
    token: &Secret,
    project: &Project,
    gl: &GitlabConfig,
    input: &TrackerInput,
) -> std::result::Result<Vec<PrRow>, CycleError> {
    let path = project_path(project, gl);
    let mut out = Vec::new();
    for (wi, branch) in &input.watched {
        let mrs = cli
            .merge_requests(token, &path, branch)
            .map_err(|e| e.to_string())?;
        out.extend(
            mrs.iter()
                .filter_map(|v| mr_row(v, *wi))
                .filter(|r| r.head == *branch),
        );
    }
    Ok(out)
}

/// Leitura de um lote: as issues lidas, as chaves que falharam e o erro
/// que derruba o ciclo.
#[derive(Default)]
struct Read {
    ok: Vec<serde_json::Value>,
    failed: Vec<(String, JiraError)>,
    fatal: Option<CycleError>,
}

/// `issue view --raw` de cada chave, `VIEW_WORKERS` por vez. A falha de uma
/// chave fica no `failed` e a leitura segue; senha recusada, jira-cli
/// sumido, configuração ausente ou `MAX_FAILS_IN_A_ROW` falhas seguidas
/// (`in_a_row`, que atravessa os lotes) param o lote e derrubam o ciclo.
fn view_all(cli: &JiraCli, token: &Secret, keys: &[String], in_a_row: &AtomicUsize) -> Read {
    let next = AtomicUsize::new(0);
    let read = Mutex::new(Read::default());
    let lock = || read.lock().unwrap_or_else(|e| e.into_inner());
    std::thread::scope(|s| {
        for _ in 0..VIEW_WORKERS.min(keys.len()) {
            s.spawn(|| loop {
                if lock().fatal.is_some() {
                    return;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(key) = keys.get(i) else { return };
                let res = cli.view(token, key);
                let mut r = lock();
                match res {
                    Ok(v) => {
                        in_a_row.store(0, Ordering::Relaxed);
                        r.ok.push(v);
                    }
                    Err(e) => {
                        let n = in_a_row.fetch_add(1, Ordering::Relaxed) + 1;
                        let fatal = match &e {
                            JiraError::Unauthorized
                            | JiraError::NotFound(_)
                            | JiraError::ConfigMissing(_) => Some(e.to_string()),
                            _ if n >= MAX_FAILS_IN_A_ROW => Some(format!(
                                "{n} leituras de issue seguidas falharam (a última, {key}: {e})"
                            )),
                            _ => None,
                        };
                        if r.fatal.is_none() {
                            r.fatal = fatal;
                        }
                        r.failed.push((key.clone(), e));
                    }
                }
            });
        }
    });
    read.into_inner().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeClaude, TestEnv};
    use ct_core::{default_type_map, ForgeConfig, TrackerConfig, WorkItemKind};
    use ct_jira::fake::{api_issue, FakeJira, FAKE_SERVER};
    use ct_ledger::SyncChange;
    use serde_json::Value;

    const SENHA: &str = "senha-de-teste";

    struct Jira {
        env: TestEnv,
        fake: FakeJira,
        sync: JiraGitlabSync,
    }

    fn jira_config(fake: &FakeJira) -> JiraConfig {
        JiraConfig {
            url: FAKE_SERVER.into(),
            project_key: "PROJ".into(),
            jql: String::new(),
            active_sprint_only: false,
            max_issues: ct_core::MAX_ISSUES_CEILING,
            credential_cmd: format!("printf '{SENHA}\\n'"),
            config_file: fake.config_file().display().to_string(),
            epic_link_field: "customfield_10601".into(),
            epic_name_field: "customfield_10604".into(),
            sprint_field: String::new(),
            type_map: default_type_map(),
            extra_fields: Vec::new(),
        }
    }

    /// O projeto do `TestEnv` vira Jira (+ GitHub) pelo pull do hub; `sync`
    /// = este é o ambiente de sync.
    fn assign(env: &TestEnv, jira: JiraConfig, sync: bool, rev: i64) {
        env.assign_provider(TrackerConfig::Jira(jira), ForgeConfig::Github, sync, rev);
    }

    fn setup_with(sync: bool, edit: impl FnOnce(&mut JiraConfig, &mut TrackerSyncConfig)) -> Jira {
        let env = TestEnv::new(FakeClaude::exit(0));
        let fake = FakeJira::new(env.tmp.path(), SENHA);
        let mut jira = jira_config(&fake);
        let mut cfg = TrackerSyncConfig::new(fake.bin().to_str().unwrap());
        edit(&mut jira, &mut cfg);
        assign(&env, jira, sync, 1);
        let sync = JiraGitlabSync::new(env.sessions.clone(), cfg);
        Jira { env, fake, sync }
    }

    fn setup(sync: bool) -> Jira {
        setup_with(sync, |_, _| {})
    }

    /// Épico (pelo tipo), Story com Epic Link e sub-task, e uma issue
    /// avulsa; todos abertos.
    fn arvore() -> Vec<Value> {
        let epico = api_issue("PROJ-1", "Épico", "new");
        let mut story = api_issue("PROJ-2", "História", "indeterminate");
        story["fields"]["customfield_10601"] = "PROJ-1".into();
        story["fields"]["subtasks"] = serde_json::json!([{ "key": "PROJ-3" }]);
        let mut sub = api_issue("PROJ-3", "Sub-tarefa", "new");
        sub["fields"]["issuetype"]["subtask"] = true.into();
        sub["fields"]["parent"] = serde_json::json!({ "key": "PROJ-2" });
        let avulsa = api_issue("PROJ-4", "Tarefa", "new");
        vec![epico, story, sub, avulsa]
    }

    fn outbox_kinds(env: &TestEnv) -> Vec<String> {
        env.sessions
            .ledger()
            .outbox_pending(1000)
            .unwrap()
            .iter()
            .map(|e| e.change.kind().to_string())
            .collect()
    }

    fn error_of(env: &TestEnv) -> String {
        env.sessions.ledger().sync_states().unwrap()[0]
            .last_error
            .clone()
            .unwrap_or_default()
    }

    #[test]
    fn epico_issue_e_sub_task_aparecem_na_arvore() {
        let j = setup(false);
        j.fake.set_issues(&arvore());
        let r = j.sync.sync_all().unwrap();
        assert_eq!(r.len(), 1);
        assert!(r[0].ok, "{r:?}");
        assert_eq!(r[0].issues, 4);

        let tree = j
            .env
            .sessions
            .ledger()
            .project_tree(j.env.project_id)
            .unwrap();
        let roots: Vec<i64> = tree.roots.iter().map(|n| n.issue.gh_number).collect();
        assert_eq!(roots, vec![1, 4]);
        let epico = &tree.roots[0];
        assert!(epico.issue.has_sub_issues);
        assert_eq!(epico.issue.url, format!("{FAKE_SERVER}/browse/PROJ-1"));
        assert_eq!(epico.children.len(), 1);
        let story = &epico.children[0];
        assert_eq!(story.issue.gh_number, 2);
        assert!(story.issue.has_sub_issues, "Story com sub-task vira épico");
        assert_eq!(story.children[0].issue.gh_number, 3);
        assert!(tree.sync.unwrap().last_ok_at.is_some());
    }

    /// "Ver issue" (#434): a descrição vem crua do `issue view`, sem passar
    /// pelo espelho nem pelo outbox; issue sem descrição e senha recusada.
    #[test]
    fn descricao_da_issue_vem_do_view_sem_gravar() {
        let j = setup(true);
        let mut i = api_issue("PROJ-7", "Tarefa", "new");
        i["fields"]["summary"] = "Corrigir login".into();
        i["fields"]["description"] = "h2. Contexto\n*a*".into();
        j.fake.set_view(&i);
        let before = outbox_kinds(&j.env);
        let v = j.sync.issue_description(j.env.project_id, 7).unwrap();
        assert_eq!(v["title"], "Corrigir login");
        assert_eq!(v["description"], "h2. Contexto\n*a*");
        assert_eq!(v["url"], format!("{FAKE_SERVER}/browse/PROJ-7"));
        assert_eq!(j.fake.viewed(), vec!["PROJ-7"]);
        assert_eq!(outbox_kinds(&j.env), before, "nada vai ao outbox");
        let tree = j
            .env
            .sessions
            .ledger()
            .project_tree(j.env.project_id)
            .unwrap();
        assert!(tree.roots.is_empty(), "nada vai ao espelho");

        let mut sem = api_issue("PROJ-8", "Tarefa", "new");
        sem["fields"]["description"] = Value::Null;
        j.fake.set_view(&sem);
        let v = j.sync.issue_description(j.env.project_id, 8).unwrap();
        assert_eq!(v["description"], Value::Null);

        j.fake.fail_view_401("PROJ-7", true);
        let e = j.sync.issue_description(j.env.project_id, 7).unwrap_err();
        assert!(e.to_string().contains("PROJ-7"), "{e}");
        assert!(!e.to_string().contains(SENHA), "{e}");
    }

    #[test]
    fn issue_que_vai_para_done_e_fechada() {
        let j = setup(false);
        let mut issues = arvore();
        j.fake.set_issues(&issues);
        assert!(j.sync.sync_all().unwrap()[0].ok);
        let wi = j
            .env
            .sessions
            .ledger()
            .ensure_work_item(j.env.project_id, 4, WorkItemKind::Issue)
            .unwrap();

        // "Concluído" (categoria done), com `updated` novo.
        issues[3] = api_issue("PROJ-4", "Tarefa", "done");
        issues[3]["fields"]["updated"] = "2026-10-28T11:00:00.000-0300".into();
        j.fake.set_issues(&issues);
        j.fake.clear_calls();
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert_eq!(j.fake.viewed(), vec!["PROJ-4"], "só a que mudou é relida");

        let l = j.env.sessions.ledger();
        assert_eq!(
            l.get_issue(j.env.project_id, 4).unwrap().unwrap().state,
            "closed"
        );
        let closed = l
            .list_events(wi.id, 50)
            .unwrap()
            .iter()
            .filter(|e| e.kind == "issue_closed")
            .count();
        assert_eq!(closed, 1);
    }

    #[test]
    fn jira_cli_ausente_gera_erro_claro() {
        let j = setup_with(false, |_, c| c.jira_bin = "/nao/existe/jira".into());
        let r = j.sync.sync_all().unwrap();
        assert!(!r[0].ok);
        let msg = "jira-cli não encontrado (jira_bin = \"/nao/existe/jira\")";
        assert_eq!(r[0].error.as_deref(), Some(msg));
        assert_eq!(error_of(&j.env), msg);
        let rd = &j.sync.readiness()[0];
        assert_eq!(rd.jira.cli, None);
        assert_eq!(rd.jira.error.as_deref(), Some(msg));
        assert_eq!(
            (rd.owner.as_str(), rd.repo.as_str()),
            ("deep-ion-ai", "demo")
        );
    }

    #[test]
    fn configuracao_ausente_ou_de_outro_servidor() {
        let j = setup_with(false, |c, _| c.config_file = "/nao/existe.yml".into());
        assert!(!j.sync.sync_all().unwrap()[0].ok);
        assert!(error_of(&j.env).contains("configuração do jira-cli ausente em /nao/existe.yml"));
        assert!(!j.sync.readiness()[0].jira.config_present);

        let j = setup_with(false, |c, _| c.url = "https://outro.jira".into());
        assert!(!j.sync.sync_all().unwrap()[0].ok);
        assert!(error_of(&j.env).contains("não é a URL do cadastro"));
        let rd = &j.sync.readiness()[0];
        assert!(rd.jira.config_present && !rd.jira.server_matches);
    }

    #[test]
    fn credencial_falhando_ou_travando_gera_erro_sem_vazar_a_senha() {
        // A saída do comando tem a senha, mas o comando não (o texto do
        // comando é do cadastro e aparece na instrução).
        let j = setup_with(false, |c, _| {
            c.credential_cmd = "printf 'senha-%s\\n' de-teste; exit 2".into()
        });
        j.fake.set_issues(&arvore());
        let r = j.sync.sync_all().unwrap();
        let msg = r[0].error.clone().unwrap();
        assert!(
            msg.contains("falhou (saída 2)") && msg.contains("gpg-agent"),
            "{msg}"
        );
        assert!(!msg.contains(SENHA));
        assert!(!j.sync.readiness()[0].jira.credential_ok);
        assert!(
            j.fake.calls().iter().all(|c| !c.contains("\tissue\t")),
            "sem senha, o Jira nem é chamado"
        );
        assert!(j
            .env
            .sessions
            .ledger()
            .list_issues(j.env.project_id)
            .unwrap()
            .is_empty());

        let j = setup_with(false, |c, s| {
            c.credential_cmd = "sleep 30".into();
            s.credential_deadline = Duration::from_millis(300);
        });
        let start = std::time::Instant::now();
        let msg = j.sync.sync_all().unwrap()[0].error.clone().unwrap();
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(
            msg.contains("não respondeu") && msg.contains("pinentry"),
            "{msg}"
        );
    }

    #[test]
    fn senha_recusada_e_jira_fora_do_ar() {
        let j = setup(false);
        j.fake.set_issues(&arvore());
        j.fake.fail_401(true);
        let r = j.sync.sync_all().unwrap();
        assert_eq!(r[0].error.as_deref(), Some("o Jira recusou a senha (401)"));
        let rd = &j.sync.readiness()[0];
        assert!(rd.jira.credential_ok && rd.jira.server_version.is_none());

        // Prazo do jira-cli estourado.
        j.fake.fail_401(false);
        j.fake.set_sleep(Some(5));
        let s = JiraGitlabSync::new(
            j.env.sessions.clone(),
            TrackerSyncConfig {
                view_deadline: Duration::from_millis(300),
                ..TrackerSyncConfig::new(j.fake.bin().to_str().unwrap())
            },
        );
        let msg = s.sync_all().unwrap()[0].error.clone().unwrap();
        assert!(msg.contains("não respondeu"), "{msg}");
        assert!(j
            .env
            .sessions
            .ledger()
            .list_issues(j.env.project_id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn token_chega_ao_jira_cli_so_pelo_ambiente() {
        let j = setup(false);
        j.fake.set_issues(&arvore());
        assert!(j.sync.sync_all().unwrap()[0].ok);
        let calls = j.fake.calls();
        assert!(calls.iter().any(|c| c.contains("\tissue\tview\t")));
        for c in &calls {
            assert!(!c.contains(SENHA), "senha no argv: {c}");
            let expected = if c.starts_with("jira\tversion\t") {
                "token=ausente"
            } else {
                "token=senha-ok"
            };
            assert!(c.contains(expected), "{c}");
            assert!(c.contains(&format!("config={}", j.fake.config_file().display())));
        }
        // Nem no estado do sync, nem nos avisos, nem na prontidão.
        let notices = format!("{:?}", j.env.notices());
        let readiness = format!("{:?}", j.sync.readiness());
        assert!(!notices.contains(SENHA) && !readiness.contains(SENHA));
    }

    #[test]
    fn prontidao_completa() {
        let j = setup(false);
        j.fake.set_issues(&arvore());
        assert!(j.sync.readiness().is_empty(), "antes do primeiro ciclo");
        j.sync.sync_all().unwrap();
        let rd = j.sync.readiness();
        assert_eq!(rd.len(), 1);
        let jr = &rd[0].jira;
        assert_eq!(jr.cli.as_deref(), Some("1.7.0"));
        assert!(jr.config_present && jr.server_matches && jr.credential_ok && jr.me_ok);
        assert_eq!(jr.server_version.as_deref(), Some("8.5.2"));
        assert_eq!(jr.error, None);
        assert!(
            jr.custom_fields.is_empty(),
            "configuração sem issue.fields.custom"
        );
        assert!(rd[0].checked_at > 0);

        // Com o catálogo do `jira init`, a prontidão o publica.
        let cfg = j.fake.config_file();
        let mut text = std::fs::read_to_string(&cfg).unwrap();
        text.push_str(concat!(
            "issue:\n",
            "    fields:\n",
            "        custom:\n",
            "            - name: Responsável pela implementação\n",
            "              key: customfield_10099\n",
            "              schema:\n",
            "                datatype: user\n",
        ));
        std::fs::write(&cfg, text).unwrap();
        j.sync.sync_all().unwrap();
        let jr = &j.sync.readiness()[0].jira;
        assert_eq!(jr.custom_fields.len(), 1);
        assert_eq!(
            (
                jr.custom_fields[0].id.as_str(),
                jr.custom_fields[0].datatype.as_str()
            ),
            ("customfield_10099", "user")
        );

        // `jira me` falhando fica anotado, sem parar o ciclo.
        j.fake.fail_me(true);
        assert!(j.sync.sync_all().unwrap()[0].ok);
        let jr = &j.sync.readiness()[0].jira;
        assert!(!jr.me_ok && jr.error.as_deref().unwrap().starts_with("jira me:"));
    }

    #[test]
    fn leitura_incremental_pede_o_filtro_por_updated() {
        let j = setup_with(false, |c, _| c.jql = "labels = fonetica".into());
        j.fake.set_issues(&arvore());
        let r = j.sync.sync_all().unwrap();
        assert!(r[0].ok, "{r:?} {:?}", j.fake.calls());
        assert_eq!(
            j.fake.list_filters(),
            vec!["(labels = fonetica) AND (statusCategory != 3 OR updated >= -30d)"]
        );
        assert_eq!(j.fake.viewed().len(), 4);

        j.fake.clear_calls();
        j.sync.sync_all().unwrap();
        let f = j.fake.list_filters();
        assert_eq!(f.len(), 1);
        // Logo depois do sucesso: 0 a 1 min + 5 de folga.
        assert!(
            f[0] == "(labels = fonetica) AND updated >= -5m"
                || f[0] == "(labels = fonetica) AND updated >= -6m",
            "{f:?}"
        );
        assert!(j.fake.viewed().is_empty(), "nada mudou: nenhuma releitura");

        // A marca da reconciliação fica no `gh_etag`.
        let mark = j
            .env
            .sessions
            .ledger()
            .etag(&full_mark_key("deep-ion-ai", "demo"))
            .unwrap()
            .unwrap();
        assert!(mark.contains("labels = fonetica"));
    }

    #[test]
    fn mudar_o_mapa_de_tipos_forca_a_reconciliacao() {
        let j = setup(false);
        j.fake.set_issues(&arvore());
        j.sync.sync_all().unwrap();
        let mut cfg = jira_config(&j.fake);
        cfg.type_map
            .insert("Tarefa".into(), ct_core::JiraTypeRole::Chore);
        assign(&j.env, cfg, false, 2);
        j.fake.clear_calls();
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert!(j.fake.list_filters()[0].contains("statusCategory != 3"));
        assert_eq!(j.fake.viewed().len(), 4, "relê tudo com o mapa novo");
        let l = j.env.sessions.ledger();
        assert_eq!(
            l.get_issue(j.env.project_id, 4).unwrap().unwrap().labels,
            vec!["chore"]
        );
    }

    /// Incluir um campo do projeto relê todas as issues (o `updated` não
    /// mudou) e o campo aparece no `meta` com o rótulo cadastrado.
    #[test]
    fn mudar_os_campos_do_projeto_rele_todas_as_issues() {
        let j = setup(false);
        let mut issues = arvore();
        issues[3]["fields"]["customfield_10099"] =
            serde_json::json!({ "name": "dev", "displayName": "Quem Implementa" });
        j.fake.set_issues(&issues);
        assert!(j.sync.sync_all().unwrap()[0].ok);
        {
            let l = j.env.sessions.ledger();
            let i = l.get_issue(j.env.project_id, 4).unwrap().unwrap();
            assert!(i.meta.iter().all(|m| m.field != "customfield_10099"));
        }
        let mut cfg = jira_config(&j.fake);
        cfg.extra_fields = vec![ct_core::JiraExtraField {
            id: "customfield_10099".into(),
            label: "Responsável pela implementação".into(),
        }];
        assign(&j.env, cfg, false, 2);
        j.fake.clear_calls();
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert_eq!(j.fake.viewed().len(), 4, "relê tudo com os campos novos");
        let l = j.env.sessions.ledger();
        let i = l.get_issue(j.env.project_id, 4).unwrap().unwrap();
        let last = i.meta.last().unwrap();
        assert_eq!(
            (
                last.field.as_str(),
                last.label.as_str(),
                last.value.as_str()
            ),
            (
                "customfield_10099",
                "Responsável pela implementação",
                "Quem Implementa"
            )
        );
        drop(l);
        j.fake.clear_calls();
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert!(j.fake.viewed().is_empty(), "{:?}", j.fake.viewed());
    }

    #[test]
    fn chave_de_outro_projeto_e_ignore_saem_do_espelho() {
        let j = setup(false);
        let mut issues = arvore();
        issues.push(api_issue("OUTRO-9", "Tarefa", "new"));
        let mut cfg = jira_config(&j.fake);
        cfg.type_map
            .insert("Tarefa".into(), ct_core::JiraTypeRole::Ignore);
        assign(&j.env, cfg, false, 2);
        j.fake.set_issues(&issues);
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert!(!j.fake.viewed().contains(&"OUTRO-9".to_string()));
        let numbers: Vec<i64> = j
            .env
            .sessions
            .ledger()
            .list_issues(j.env.project_id)
            .unwrap()
            .iter()
            .map(|i| i.gh_number)
            .collect();
        assert_eq!(numbers, vec![1, 2, 3]);
    }

    #[test]
    fn so_o_ambiente_de_sync_enfileira_o_espelho() {
        let j = setup(false);
        j.fake.set_issues(&arvore());
        j.env.sessions.ledger().outbox_ack(i64::MAX).unwrap();
        assert!(j.sync.sync_all().unwrap()[0].ok);
        let kinds = outbox_kinds(&j.env);
        assert!(
            !kinds
                .iter()
                .any(|k| k == "issue_mirror" || k == "tracker_status"),
            "{kinds:?}"
        );

        let j = setup(true);
        j.fake.set_issues(&arvore());
        j.env.sessions.ledger().outbox_ack(i64::MAX).unwrap();
        assert!(j.sync.sync_all().unwrap()[0].ok);
        let out = j.env.sessions.ledger().outbox_pending(1000).unwrap();
        let mirrored = out
            .iter()
            .filter(|e| matches!(e.change, SyncChange::IssueMirror(_)))
            .count();
        assert_eq!(mirrored, 4);
        let st: Vec<_> = out
            .iter()
            .filter_map(|e| match &e.change {
                SyncChange::TrackerStatus(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(st.len(), 1);
        assert!(st[0].last_ok_at.is_some() && st[0].last_error.is_none());
        assert_eq!(
            st[0].readiness.jira.server_version.as_deref(),
            Some("8.5.2")
        );

        // Ciclo sem mudança: nenhum `issue_mirror` novo; o status vai sempre,
        // também com erro.
        j.env.sessions.ledger().outbox_ack(i64::MAX).unwrap();
        j.fake.fail_401(true);
        assert!(!j.sync.sync_all().unwrap()[0].ok);
        let out = j.env.sessions.ledger().outbox_pending(1000).unwrap();
        assert_eq!(out.len(), 1, "{out:?}");
        let SyncChange::TrackerStatus(t) = &out[0].change else {
            panic!("{out:?}");
        };
        assert_eq!(
            t.last_error.as_deref(),
            Some("o Jira recusou a senha (401)")
        );
        assert!(t.last_ok_at.is_some(), "o último sucesso continua");
    }

    #[test]
    fn projeto_github_nao_e_deste_laco() {
        let env = TestEnv::new(FakeClaude::exit(0));
        let fake = FakeJira::new(env.tmp.path(), SENHA);
        let sync = JiraGitlabSync::new(
            env.sessions.clone(),
            TrackerSyncConfig::new(fake.bin().to_str().unwrap()),
        );
        assert!(!sync.handles(env.project_id));
        assert!(sync.sync_all().unwrap().is_empty());
        assert!(fake.calls().is_empty());
        let r = sync.sync_project(env.project_id).unwrap();
        assert!(!r.ok && r.error.as_deref().unwrap().contains("sem rastreador Jira"));
    }

    /// `n` tarefas abertas, `PROJ-1` a `PROJ-<n>`.
    fn tarefas(n: i64) -> Vec<Value> {
        (1..=n)
            .map(|i| api_issue(&format!("PROJ-{i}"), "Tarefa", "new"))
            .collect()
    }

    fn numbers(env: &TestEnv) -> Vec<i64> {
        env.sessions
            .ledger()
            .list_issues(env.project_id)
            .unwrap()
            .iter()
            .map(|i| i.gh_number)
            .collect()
    }

    #[test]
    fn lotes_ficam_gravados_e_o_ciclo_interrompido_recomeca() {
        // Sem limite (sprint ativa): a listagem em ordem crescente.
        let j = setup_with(true, |jc, c| {
            jc.active_sprint_only = true;
            c.batch_size = 2;
        });
        j.fake.set_issues(&tarefas(5));
        // A senha passa a ser recusada no meio do ciclo: o 3º lote cai.
        j.fake.fail_view_401("PROJ-5", true);
        j.env.sessions.ledger().outbox_ack(i64::MAX).unwrap();
        let r = j.sync.sync_all().unwrap();
        assert!(!r[0].ok);
        assert_eq!(r[0].error.as_deref(), Some("o Jira recusou a senha (401)"));
        assert_eq!(numbers(&j.env), vec![1, 2, 3, 4], "os dois lotes ficaram");
        let l = j.env.sessions.ledger();
        assert_eq!(
            l.sync_state(j.env.project_id).unwrap().unwrap().last_ok_at,
            None
        );
        assert_eq!(l.etag(&full_mark_key("deep-ion-ai", "demo")).unwrap(), None);
        // Os lotes subiram ao hub; o status final já sem progresso.
        let out = l.outbox_pending(1000).unwrap();
        let mirrored = out
            .iter()
            .filter(|e| matches!(e.change, SyncChange::IssueMirror(_)))
            .count();
        assert_eq!(mirrored, 4);
        let st: Vec<_> = out
            .iter()
            .filter_map(|e| match &e.change {
                SyncChange::TrackerStatus(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(st.len(), 1);
        assert_eq!(st[0].readiness.progress, None);
        drop(l);
        assert_eq!(j.sync.readiness()[0].progress, None);

        // O próximo ciclo é de novo a reconciliação e lê só o que faltou.
        j.fake.fail_view_401("PROJ-5", false);
        j.fake.clear_calls();
        let r = j.sync.sync_all().unwrap();
        assert!(r[0].ok && r[0].error.is_none(), "{r:?}");
        assert!(j.fake.list_filters()[0].contains("statusCategory != 3"));
        assert_eq!(j.fake.viewed(), vec!["PROJ-5"]);
        assert_eq!(numbers(&j.env), vec![1, 2, 3, 4, 5]);
        let l = j.env.sessions.ledger();
        assert!(l
            .etag(&full_mark_key("deep-ion-ai", "demo"))
            .unwrap()
            .is_some());
        assert!(l
            .sync_state(j.env.project_id)
            .unwrap()
            .unwrap()
            .last_ok_at
            .is_some());
    }

    #[test]
    fn leitura_entrega_lotes_com_o_progresso() {
        let j = setup(false);
        let mut issues = arvore();
        issues.push(api_issue("PROJ-5", "Tarefa", "new"));
        j.fake.set_issues(&issues);
        let (cfg, project, input) = {
            let l = j.env.sessions.ledger();
            let p = l.get_project(j.env.project_id).unwrap();
            let input = TrackerInput::load(&l, &p).unwrap();
            (p.tracker.jira().cloned().unwrap(), p, input)
        };
        let cli = JiraCli::new(j.fake.bin().to_str().unwrap(), &cfg.config_file);
        let mut calls: Vec<(Vec<i64>, usize, usize)> = Vec::new();
        let f = fetch(
            &cli,
            &Secret::new(SENHA),
            &cfg,
            &project,
            &input,
            now_ms(),
            2,
            &mut |rows: Vec<IssueRow>, done, total| {
                calls.push((rows.iter().map(|r| r.gh_number).collect(), done, total));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(f.total, 5);
        assert!(f.failed.is_empty());
        assert_eq!(calls[0], (vec![], 5, 0), "a listagem, sem total");
        assert_eq!(calls[1], (vec![], 0, 5), "o início, com o total");
        assert_eq!(
            calls.iter().map(|c| (c.1, c.2)).collect::<Vec<_>>(),
            vec![(5, 0), (0, 5), (2, 5), (4, 5), (5, 5)]
        );
        let mut lidas: Vec<i64> = calls.iter().flat_map(|c| c.0.clone()).collect();
        lidas.sort();
        assert_eq!(lidas, vec![1, 2, 3, 4, 5]);

        // O progresso vai já na prontidão e, no máximo a cada 30 s, no
        // outbox (só no ambiente de sync).
        let j = setup(true);
        j.env.sessions.ledger().outbox_ack(i64::MAX).unwrap();
        let base = TrackerReadiness {
            owner: "deep-ion-ai".into(),
            repo: "demo".into(),
            ..TrackerReadiness::default()
        };
        let p = |done| TrackerProgress {
            done,
            total: 10,
            started_at: 1,
        };
        let mut last = None;
        j.sync
            .progress(j.env.project_id, &base, p(0), &mut last)
            .unwrap();
        j.sync
            .progress(j.env.project_id, &base, p(4), &mut last)
            .unwrap();
        assert_eq!(j.sync.readiness()[0].progress, Some(p(4)));
        let out = j.env.sessions.ledger().outbox_pending(1000).unwrap();
        assert_eq!(out.len(), 1, "{out:?}");
        let SyncChange::TrackerStatus(t) = &out[0].change else {
            panic!("{out:?}");
        };
        assert_eq!(t.readiness.progress, Some(p(0)), "o segundo esperou");
    }

    #[test]
    fn falha_de_uma_issue_nao_derruba_o_ciclo_e_ela_e_lida_de_novo() {
        let j = setup(false);
        let issues = arvore();
        j.fake.set_issues(&issues);
        j.fake.remove_view("PROJ-4");
        let r = j.sync.sync_all().unwrap();
        assert!(r[0].ok, "{r:?}");
        let w = r[0].error.clone().unwrap();
        assert!(
            w.starts_with("1 issue(s) não lidas") && w.contains("PROJ-4"),
            "{w}"
        );
        assert!(w.contains("404"), "{w}");
        assert_eq!(error_of(&j.env), w);
        assert_eq!(numbers(&j.env), vec![1, 2, 3]);
        let l = j.env.sessions.ledger();
        assert!(l
            .sync_state(j.env.project_id)
            .unwrap()
            .unwrap()
            .last_ok_at
            .is_some());
        assert_eq!(
            l.etag(&retry_key("deep-ion-ai", "demo"))
                .unwrap()
                .as_deref(),
            Some("PROJ-4")
        );
        drop(l);

        // Incremental: a issue nem aparece no recorte por `updated`, mas é
        // lida de novo pela lista das que faltaram.
        j.fake.set_issues(&issues[..3]);
        j.fake.set_view(&issues[3]);
        j.fake.clear_calls();
        let r = j.sync.sync_all().unwrap();
        assert!(r[0].ok && r[0].error.is_none(), "{r:?}");
        assert!(j.fake.list_filters()[0].contains("updated >= -"));
        assert_eq!(j.fake.viewed(), vec!["PROJ-4"]);
        assert_eq!(numbers(&j.env), vec![1, 2, 3, 4]);
        assert_eq!(error_of(&j.env), "");
        let l = j.env.sessions.ledger();
        assert_eq!(
            l.etag(&retry_key("deep-ion-ai", "demo"))
                .unwrap()
                .as_deref(),
            Some("")
        );
    }

    #[test]
    fn pagina_da_listagem_falhando_e_pedida_de_novo() {
        let j = setup_with(false, |_, c| {
            c.list_deadline = Duration::from_secs(2);
            c.list_retry_waits = vec![Duration::from_millis(10); 2];
        });
        j.fake.set_issues(&tarefas(150));
        // A segunda página estoura o prazo uma vez e responde na seguinte.
        j.fake.flaky_page(1, 1, "sleep:6");
        let r = j.sync.sync_all().unwrap();
        assert!(r[0].ok && r[0].error.is_none(), "{r:?}");
        assert_eq!((j.fake.page_calls(0), j.fake.page_calls(100)), (1, 2));
        assert_eq!(numbers(&j.env).len(), 150);

        // Três falhas seguidas na mesma página derrubam o ciclo.
        j.fake.clear_calls();
        j.fake.flaky_page(0, 3, "503");
        let r = j.sync.sync_all().unwrap();
        assert!(!r[0].ok);
        let e = r[0].error.clone().unwrap();
        assert!(e.contains("Jira fora do ar") && e.contains("503"), "{e}");
        assert_eq!(j.fake.page_calls(0), 3);
        assert!(j.fake.viewed().is_empty());
    }

    #[test]
    fn prazos_da_listagem_e_do_view_vem_da_configuracao() {
        let c = TrackerSyncConfig::new("jira")
            .with_jira_deadlines(Duration::from_secs(45), Duration::from_secs(300));
        assert_eq!(c.view_deadline, Duration::from_secs(45));
        assert_eq!(c.list_deadline, Duration::from_secs(300));
        assert_eq!(c.gitlab().cli_deadline, ct_gitlab::DEADLINE);
        let d = TrackerSyncConfig::new("jira");
        assert_eq!(
            (d.view_deadline, d.list_deadline),
            (DEADLINE, LIST_DEADLINE)
        );
        assert_eq!(d.list_retry_waits.len(), 2);
    }

    #[test]
    fn falhas_seguidas_derrubam_o_ciclo() {
        let j = setup(false);
        let issues = tarefas(MAX_FAILS_IN_A_ROW as i64 + 2);
        j.fake.set_issues(&issues);
        for v in &issues {
            j.fake.remove_view(v["key"].as_str().unwrap());
        }
        let r = j.sync.sync_all().unwrap();
        assert!(!r[0].ok);
        let e = r[0].error.clone().unwrap();
        assert!(e.contains("leituras de issue seguidas falharam"), "{e}");
        let l = j.env.sessions.ledger();
        assert_eq!(
            l.sync_state(j.env.project_id).unwrap().unwrap().last_ok_at,
            None
        );
        assert_eq!(l.etag(&retry_key("deep-ion-ai", "demo")).unwrap(), None);
    }

    // GitLab (#350): MRs pelo glab no mesmo ciclo.

    const HOST: &str = "git.exemplo.test";
    const PATH: &str = "deep-ion-ai/demo";
    const TOKEN: &str = "token-de-teste";

    struct Gitlab {
        j: Jira,
        glab: ct_gitlab::fake::FakeGlab,
    }

    fn gitlab_config() -> ct_core::GitlabConfig {
        ct_core::GitlabConfig {
            host: HOST.into(),
            path: PATH.into(),
            clone_url: format!("https://{HOST}/{PATH}.git"),
            credential_cmd: format!("printf '{TOKEN}\\n'"),
        }
    }

    fn setup_gitlab(sync: bool, edit: impl FnOnce(&mut ct_core::GitlabConfig)) -> Gitlab {
        let env = TestEnv::new(FakeClaude::exit(0));
        let fake = FakeJira::new(env.tmp.path(), SENHA);
        let glab = ct_gitlab::fake::FakeGlab::new(env.tmp.path(), HOST, TOKEN);
        let mut gl = gitlab_config();
        edit(&mut gl);
        env.assign_provider(
            TrackerConfig::Jira(jira_config(&fake)),
            ForgeConfig::Gitlab(gl),
            sync,
            1,
        );
        let cfg = TrackerSyncConfig::new(fake.bin().to_str().unwrap())
            .with_glab_bin(glab.bin().to_str().unwrap());
        let sync = JiraGitlabSync::new(env.sessions.clone(), cfg);
        fake.set_issues(&arvore());
        Gitlab {
            j: Jira { env, fake, sync },
            glab,
        }
    }

    impl Gitlab {
        /// Work item da PROJ-4, em execução, com a branch `PROJ-4`.
        fn work_item(&self) -> i64 {
            let l = self.j.env.sessions.ledger();
            let wi = l
                .ensure_work_item(self.j.env.project_id, 4, WorkItemKind::Issue)
                .unwrap();
            l.set_work_item_branch(wi.id, "PROJ-4", "").unwrap();
            l.set_work_item_status(wi.id, ct_core::WorkItemStatus::EmExecucao, "teste")
                .unwrap();
            wi.id
        }

        fn mr(&self, state: &str) {
            self.glab.set_mrs(
                "PROJ-4",
                &[ct_gitlab::fake::api_mr(
                    HOST, PATH, 7, "PROJ-4", state, "abc123",
                )],
            );
        }

        fn status(&self, wi: i64) -> ct_core::WorkItemStatus {
            self.j
                .env
                .sessions
                .ledger()
                .get_work_item(wi)
                .unwrap()
                .status
        }

        fn completion(&self, wi: i64) -> Option<crate::cleanup::Completion> {
            let l = self.j.env.sessions.ledger();
            let p = l.get_project(self.j.env.project_id).unwrap();
            let w = l.get_work_item(wi).unwrap();
            crate::cleanup::completion(&l, &p, &w).unwrap()
        }
    }

    /// Critério de aceite da #350: o MR aberto leva a "em revisão" e o
    /// mesclado à conclusão, pelo mesmo caminho do PR (`PrMesclado`).
    #[test]
    fn mr_mesclado_leva_a_conclusao() {
        let g = setup_gitlab(false, |_| {});
        let wi = g.work_item();
        g.mr("opened");
        let r = g.j.sync.sync_all().unwrap();
        assert!(r[0].ok, "{r:?}");
        assert_eq!((r[0].prs, r[0].status_changes), (1, 1));
        assert_eq!(g.status(wi), ct_core::WorkItemStatus::EmRevisao);
        let prs = g.j.env.sessions.ledger().prs_for_work_item(wi).unwrap();
        assert_eq!(prs[0].number, 7, "o iid do MR");
        assert_eq!(
            prs[0].url,
            format!("https://{HOST}/{PATH}/-/merge_requests/7")
        );
        assert_eq!(prs[0].head_sha.as_deref(), Some("abc123"));

        g.mr("merged");
        assert!(g.j.sync.sync_all().unwrap()[0].ok);
        // O gatilho do MR mesclado conclui o work item (como o PR), e a
        // conclusão vale para a limpeza.
        assert_eq!(g.status(wi), ct_core::WorkItemStatus::Concluido);
        let c = g.completion(wi).unwrap();
        assert_eq!(
            c.signal,
            crate::cleanup::CompletionSignal::WorkItemConcluido
        );
        let events = g.j.env.sessions.ledger().list_events(wi, 50).unwrap();
        assert!(
            events
                .iter()
                .any(|e| e.uid == "gh:pr:deep-ion-ai/demo#7:merged"),
            "{events:?}"
        );
        assert_eq!(
            crate::cleanup::CompletionSignal::PrMesclado.label_for(ct_core::ForgeKind::Gitlab),
            "MR mesclado"
        );

        // Concluído com o MR mesclado: o ciclo seguinte nem pergunta.
        g.glab.clear_calls();
        assert!(g.j.sync.sync_all().unwrap()[0].ok);
        assert!(
            g.glab.calls().iter().all(|c| !c.contains("\tmr\tlist\t")),
            "{:?}",
            g.glab.calls()
        );
    }

    /// Critério de aceite da #350: o MR fechado sem merge dá o sinal de
    /// fechado (`PrFechado`), sem concluir o work item.
    #[test]
    fn mr_fechado_da_o_sinal_de_fechado() {
        let g = setup_gitlab(false, |_| {});
        let wi = g.work_item();
        g.mr("closed");
        assert!(g.j.sync.sync_all().unwrap()[0].ok);
        assert_eq!(g.status(wi), ct_core::WorkItemStatus::EmExecucao);
        let c = g.completion(wi).unwrap();
        assert_eq!(
            (c.signal, c.pr),
            (crate::cleanup::CompletionSignal::PrFechado, Some(7))
        );
        assert_eq!(c.signal.label_for(ct_core::ForgeKind::Gitlab), "MR fechado");
    }

    /// Critério de aceite da #350: o ambiente de sync enfileira o MR como
    /// `pr_mirror`; os outros, não.
    #[test]
    fn ambiente_de_sync_enfileira_pr_mirror() {
        for sync in [true, false] {
            let g = setup_gitlab(sync, |_| {});
            let wi = g.work_item();
            g.mr("opened");
            g.j.env.sessions.ledger().outbox_ack(i64::MAX).unwrap();
            assert!(g.j.sync.sync_all().unwrap()[0].ok);
            let out = g.j.env.sessions.ledger().outbox_pending(1000).unwrap();
            let mrs: Vec<_> = out
                .iter()
                .filter_map(|e| match &e.change {
                    SyncChange::PrMirror(m) => Some(m),
                    _ => None,
                })
                .collect();
            if !sync {
                assert!(mrs.is_empty(), "{mrs:?}");
                continue;
            }
            assert_eq!(mrs.len(), 1);
            assert_eq!((mrs[0].gh_number, mrs[0].pr.number), (4, 7));
            assert_eq!(mrs[0].pr.head, "PROJ-4");
            let st = out
                .iter()
                .find_map(|e| match &e.change {
                    SyncChange::TrackerStatus(t) => Some(t),
                    _ => None,
                })
                .unwrap();
            assert!(st.readiness.gitlab.api_ok);
            let _ = wi;
        }
    }

    /// Critério de aceite da #350: o token do GitLab chega ao glab só pelo
    /// ambiente do processo, com o host do cadastro, e não aparece em
    /// argv, estado do sync, avisos nem prontidão.
    #[test]
    fn token_do_gitlab_so_no_ambiente_do_glab() {
        let g = setup_gitlab(true, |_| {});
        g.work_item();
        g.mr("opened");
        assert!(g.j.sync.sync_all().unwrap()[0].ok);
        let calls = g.glab.calls();
        assert!(
            calls.iter().any(|c| c.contains("\tmr\tlist\t")),
            "{calls:?}"
        );
        for c in &calls {
            assert!(!c.contains(TOKEN), "token no argv: {c}");
            assert!(c.contains(&format!("host={HOST}")), "{c}");
            let expected = if c.starts_with("glab\tversion\t") {
                "token=ausente"
            } else {
                "token=ok"
            };
            assert!(c.contains(expected), "{c}");
        }
        let notices = format!("{:?}", g.j.env.notices());
        let readiness = format!("{:?}", g.j.sync.readiness());
        let outbox = format!(
            "{:?}",
            g.j.env.sessions.ledger().outbox_pending(1000).unwrap()
        );
        for text in [notices, readiness, outbox] {
            assert!(!text.contains(TOKEN));
        }
        let gr = &g.j.sync.readiness()[0].gitlab;
        assert_eq!(gr.cli.as_deref(), Some("1.124.0"));
        assert!(
            gr.credential_ok && gr.api_ok && gr.error.is_none(),
            "{gr:?}"
        );
    }

    /// GitLab recusando o token, glab ausente ou credencial falhando: o
    /// ciclo falha inteiro (nem as issues do Jira entram), com o motivo e
    /// sem o token.
    #[test]
    fn gitlab_em_erro_derruba_o_ciclo() {
        let g = setup_gitlab(false, |_| {});
        g.glab.fail_401(true);
        let r = g.j.sync.sync_all().unwrap();
        assert_eq!(
            r[0].error.as_deref(),
            Some("o GitLab recusou o token (401)")
        );
        let gr = &g.j.sync.readiness()[0];
        assert!(gr.gitlab.credential_ok && !gr.gitlab.api_ok);
        assert!(gr.jira.credential_ok, "o Jira foi conferido também");
        assert!(g
            .j
            .env
            .sessions
            .ledger()
            .list_issues(g.j.env.project_id)
            .unwrap()
            .is_empty());

        let g = setup_gitlab(false, |c| {
            c.credential_cmd = format!("printf '{TOKEN}\\n'; exit 2")
        });
        let msg = g.j.sync.sync_all().unwrap()[0].error.clone().unwrap();
        assert!(
            msg.starts_with("comando de credencial do GitLab falhou (saída 2)"),
            "{msg}"
        );
        assert!(!msg.contains("token-de-teste\n") && !g.j.sync.readiness()[0].gitlab.credential_ok);

        let g = setup_gitlab(false, |_| {});
        let s = JiraGitlabSync::new(
            g.j.env.sessions.clone(),
            TrackerSyncConfig::new(g.j.fake.bin().to_str().unwrap())
                .with_glab_bin("/nao/existe/glab"),
        );
        let msg = s.sync_all().unwrap()[0].error.clone().unwrap();
        assert_eq!(msg, "glab não encontrado (glab_bin = \"/nao/existe/glab\")");
        assert_eq!(s.readiness()[0].gitlab.cli, None);
    }

    /// Sprint do Jira 8.5.2 (texto do GreenHopper), com o `<null>` escapado.
    fn sprint_gh(id: i64, state: &str, name: &str) -> String {
        format!(
            "com.atlassian.greenhopper.service.sprint.Sprint@1f[id={id},rapidViewId=402,\
             state={state},name={name},startDate=2026-10-26T11:37:15.379-02:00,\
             endDate=2026-11-06T21:37:00.000-02:00,completeDate=&lt;null&gt;,sequence={id},goal=]"
        )
    }

    #[test]
    fn sprints_vao_ao_cache_e_ao_espelho_do_hub() {
        let j = setup(true);
        let mut issues = arvore();
        issues[3]["fields"]["customfield_10600"] = serde_json::json!([
            sprint_gh(2254, "CLOSED", "Sprint 47"),
            sprint_gh(2273, "ACTIVE", "Sprint 48"),
        ]);
        j.fake.set_issues(&issues);
        j.env.sessions.ledger().outbox_ack(i64::MAX).unwrap();
        assert!(j.sync.sync_all().unwrap()[0].ok);
        let l = j.env.sessions.ledger();
        let i = l.get_issue(j.env.project_id, 4).unwrap().unwrap();
        let names: Vec<&str> = i.sprints.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["Sprint 47", "Sprint 48"]);
        assert_eq!(i.sprints[1].state, ct_core::SprintState::Active);
        assert!(l
            .get_issue(j.env.project_id, 1)
            .unwrap()
            .unwrap()
            .sprints
            .is_empty());
        let mirrored = l
            .outbox_pending(1000)
            .unwrap()
            .into_iter()
            .find_map(|e| match e.change {
                SyncChange::IssueMirror(m) if m.issue.gh_number == 4 => Some(m.issue),
                _ => None,
            })
            .unwrap();
        assert_eq!(mirrored.sprints, i.sprints);
    }

    /// Depois do sync, os 5 campos padrão estão em `meta`, na ordem e sem
    /// os vazios, no cache e no `issue_mirror` que sobe ao hub.
    #[test]
    fn metadados_vao_ao_cache_e_ao_espelho_do_hub() {
        let j = setup(true);
        let mut issues = arvore();
        issues[3]["fields"]["assignee"] =
            serde_json::json!({ "name": "fulano", "displayName": "Fulano de Tal" });
        j.fake.set_issues(&issues);
        j.env.sessions.ledger().outbox_ack(i64::MAX).unwrap();
        assert!(j.sync.sync_all().unwrap()[0].ok);
        let l = j.env.sessions.ledger();
        let i = l.get_issue(j.env.project_id, 4).unwrap().unwrap();
        let meta: Vec<(&str, &str)> = i
            .meta
            .iter()
            .map(|m| (m.label.as_str(), m.value.as_str()))
            .collect();
        assert_eq!(
            meta,
            [
                ("Situação", "Aberto"),
                ("Tipo", "Tarefa"),
                ("Prioridade", "Média"),
                ("Responsável", "Fulano de Tal"),
                ("Criador", "Autor da Issue"),
            ]
        );
        // Sem responsável: o campo fica de fora.
        let sem = l.get_issue(j.env.project_id, 1).unwrap().unwrap();
        assert!(sem.meta.iter().all(|m| m.field != "assignee"));
        assert_eq!(sem.meta.len(), 4);
        let mirrored = l
            .outbox_pending(1000)
            .unwrap()
            .into_iter()
            .find_map(|e| match e.change {
                SyncChange::IssueMirror(m) if m.issue.gh_number == 4 => Some(m.issue),
                _ => None,
            })
            .unwrap();
        assert_eq!(mirrored.meta, i.meta);
    }

    /// O espelho gravado com a versão 2 (sprints, sem metadados) é relido
    /// uma vez por causa do `MIRROR_VERSION`, e só uma.
    #[test]
    fn espelho_sem_metadados_e_relido_uma_vez() {
        let j = setup(true);
        let mut issues = arvore();
        j.fake.set_issues(&issues);
        assert!(j.sync.sync_all().unwrap()[0].ok);
        // Como ficaria a marca gravada pela versão 2.
        let key = full_mark_key("deep-ion-ai", "demo");
        {
            let l = j.env.sessions.ledger();
            let mark = l.etag(&key).unwrap().unwrap();
            let atual = format!("\"mirror\":{MIRROR_VERSION}");
            assert!(mark.contains(&atual), "{mark}");
            let antiga = mark.replace(&atual, "\"mirror\":2");
            l.apply_github(&GithubSnapshot {
                project_id: j.env.project_id,
                etags: vec![(key.clone(), antiga)],
                synced_at: now_ms(),
                ..GithubSnapshot::default()
            })
            .unwrap();
            l.outbox_ack(i64::MAX).unwrap();
        }
        // O responsável mudou no Jira sem mudar o `updated`.
        issues[1]["fields"]["assignee"] = serde_json::json!({ "name": "fulano" });
        j.fake.set_issues(&issues);
        j.fake.clear_calls();
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert_eq!(j.fake.viewed().len(), 4, "relê tudo uma vez");
        let l = j.env.sessions.ledger();
        let i = l.get_issue(j.env.project_id, 2).unwrap().unwrap();
        assert!(i
            .meta
            .iter()
            .any(|m| m.field == "assignee" && m.value == "fulano"));
        drop(l);
        j.fake.clear_calls();
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert!(j.fake.viewed().is_empty(), "{:?}", j.fake.viewed());
    }

    /// O espelho gravado antes das sprints (marca da reconciliação com a
    /// configuração de antes, sem `mirror`) é relido uma vez, e só uma.
    #[test]
    fn espelho_de_versao_anterior_e_relido_uma_vez() {
        let j = setup(true);
        let mut issues = arvore();
        j.fake.set_issues(&issues);
        assert!(j.sync.sync_all().unwrap()[0].ok);
        // Como ficaria a marca gravada pela versão anterior.
        let key = full_mark_key("deep-ion-ai", "demo");
        {
            let l = j.env.sessions.ledger();
            let cfg = l
                .get_project(j.env.project_id)
                .unwrap()
                .tracker
                .jira()
                .cloned()
                .unwrap();
            let antiga = json!({
                "project_key": cfg.project_key,
                "jql": cfg.jql.trim(),
                "type_map": cfg.type_map,
                "epic_link_field": cfg.epic_link_field,
                "epic_name_field": cfg.epic_name_field,
                "url": cfg.url,
            });
            l.apply_github(&GithubSnapshot {
                project_id: j.env.project_id,
                etags: vec![(key.clone(), format!("{} {antiga}", now_ms()))],
                synced_at: now_ms(),
                ..GithubSnapshot::default()
            })
            .unwrap();
            l.outbox_ack(i64::MAX).unwrap();
        }
        // As issues ganharam sprint no Jira sem mudar o `updated`.
        issues[1]["fields"]["customfield_10600"] =
            serde_json::json!([sprint_gh(2273, "ACTIVE", "Sprint 48")]);
        j.fake.set_issues(&issues);
        j.fake.clear_calls();
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert_eq!(j.fake.viewed().len(), 4, "relê tudo uma vez");
        let l = j.env.sessions.ledger();
        assert_eq!(
            l.get_issue(j.env.project_id, 2).unwrap().unwrap().sprints[0].id,
            2273
        );
        // Só a issue que mudou sobe ao hub.
        let mirrored: Vec<i64> = l
            .outbox_pending(1000)
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.change {
                SyncChange::IssueMirror(m) => Some(m.issue.gh_number),
                _ => None,
            })
            .collect();
        assert_eq!(mirrored, [2]);
        assert!(l.etag(&key).unwrap().unwrap().contains("\"mirror\":"));
        drop(l);
        // O ciclo seguinte não relê de novo.
        j.fake.clear_calls();
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert!(j.fake.viewed().is_empty(), "{:?}", j.fake.viewed());
    }

    #[test]
    fn recorte_do_ciclo() {
        let cfg = JiraConfig {
            project_key: "P".into(),
            ..JiraConfig::default()
        };
        let fp = fingerprint(&cfg);
        let now = 10 * FULL_EVERY_MS;
        let input = |mark: Option<(i64, String)>, last_ok: Option<i64>| TrackerInput {
            cached: HashMap::new(),
            mark,
            last_ok_at: last_ok,
            retry: Vec::new(),
            pinned: Vec::new(),
            work_items: HashSet::new(),
            watched: Vec::new(),
        };
        // Primeiro ciclo: reconciliação.
        let p = plan(&cfg, &input(None, None), now);
        assert_eq!(p.jql, "(statusCategory != 3 OR updated >= -30d)");
        assert!(!p.reread_all && p.mark.is_some());
        // 12 min e 1 s desde o sucesso: 13 + 5.
        let p = plan(
            &cfg,
            &input(Some((now - 3_600_000, fp.clone())), Some(now - 721_000)),
            now,
        );
        assert_eq!(p.jql, "updated >= -18m");
        assert_eq!(p.mark, None);
        // Reconciliação diária.
        let p = plan(
            &cfg,
            &input(Some((now - FULL_EVERY_MS, fp.clone())), Some(now - 60_000)),
            now,
        );
        assert!(p.jql.contains("statusCategory") && !p.reread_all);
        // Configuração mudou: relê tudo.
        let p = plan(&cfg, &input(Some((now, "x".into())), Some(now)), now);
        assert!(p.jql.contains("statusCategory") && p.reread_all);
        // Só o JQL mudou: reconciliação com poda, sem reler o que não mudou.
        let novo = JiraConfig {
            jql: "labels = web".into(),
            ..cfg.clone()
        };
        let p = plan(&novo, &input(Some((now, fp.clone())), Some(now)), now);
        assert_eq!(
            p.jql,
            "(labels = web) AND (statusCategory != 3 OR updated >= -30d)"
        );
        assert!(!p.reread_all && p.prune && p.filter_changed);
        // Reconciliação diária com o mesmo JQL: poda, com a proteção.
        let fp_novo = fingerprint(&novo);
        let p = plan(
            &novo,
            &input(Some((now - FULL_EVERY_MS, fp_novo)), Some(now - 60_000)),
            now,
        );
        assert!(p.prune && !p.filter_changed && !p.reread_all);
        // Cadastro anterior sem JQL: poda pelo limite; o incremental nunca
        // poda.
        let p = plan(&cfg, &input(None, None), now);
        assert!(p.prune);
        let p = plan(
            &cfg,
            &input(Some((now - 3_600_000, fp.clone())), Some(now - 721_000)),
            now,
        );
        assert!(!p.prune && !p.filter_changed);
        // Marca de antes do modo e do limite (só com o `jql`): o filtro
        // mudou (a primeira poda pelo limite passa pela proteção), sem reler.
        let mut antiga: serde_json::Value = serde_json::from_str(&fingerprint(&novo)).unwrap();
        antiga.as_object_mut().unwrap().remove("active_sprint_only");
        antiga.as_object_mut().unwrap().remove("max_issues");
        let p = plan(
            &novo,
            &input(Some((now - 60_000, antiga.to_string())), Some(now - 60_000)),
            now,
        );
        assert!(p.prune && p.filter_changed && !p.reread_all, "{p:?}");
        assert!(p.mark.is_some(), "reconciliação");
        // Modo sprint ativa: o JQL do cadastro é ignorado; mudar o modo ou o
        // limite é mudança de filtro.
        let sprint = JiraConfig {
            active_sprint_only: true,
            ..novo.clone()
        };
        let p = plan(&sprint, &input(None, None), now);
        assert_eq!(
            p.jql,
            "(sprint in openSprints()) AND (statusCategory != 3 OR updated >= -30d)"
        );
        assert!(p.prune);
        let p = plan(
            &sprint,
            &input(Some((now, fingerprint(&novo))), Some(now)),
            now,
        );
        assert!(p.filter_changed && !p.reread_all);
        let limite = JiraConfig {
            max_issues: 50,
            ..novo.clone()
        };
        let p = plan(
            &limite,
            &input(Some((now, fingerprint(&novo))), Some(now)),
            now,
        );
        assert!(p.filter_changed && !p.reread_all && p.mark.is_some());
    }

    /// Mudar os campos do projeto (incluir, trocar o rótulo, reordenar ou
    /// remover) relê todas as issues; o projeto sem eles mantém a marca.
    #[test]
    fn campos_do_projeto_releem_tudo() {
        let extra = |id: &str, label: &str| ct_core::JiraExtraField {
            id: id.into(),
            label: label.into(),
        };
        let cfg = JiraConfig {
            project_key: "P".into(),
            ..JiraConfig::default()
        };
        assert!(read_fingerprint(&cfg).get("extra_fields").is_none());
        let now = 10 * FULL_EVERY_MS;
        let input = |fp: String| TrackerInput {
            cached: HashMap::new(),
            mark: Some((now - 60_000, fp)),
            last_ok_at: Some(now - 60_000),
            retry: Vec::new(),
            pinned: Vec::new(),
            work_items: HashSet::new(),
            watched: Vec::new(),
        };
        let com = |fields: Vec<ct_core::JiraExtraField>| JiraConfig {
            extra_fields: fields,
            ..cfg.clone()
        };
        let um = com(vec![extra("customfield_1", "A")]);
        let p = plan(&um, &input(fingerprint(&cfg)), now);
        assert!(
            p.reread_all && p.mark.is_some() && !p.filter_changed,
            "{p:?}"
        );
        // Mesma lista (espaços à parte): incremental, sem reler.
        let igual = com(vec![extra(" customfield_1 ", "A ")]);
        let p = plan(&igual, &input(fingerprint(&um)), now);
        assert!(!p.reread_all && p.mark.is_none(), "{p:?}");
        for outro in [
            com(vec![extra("customfield_1", "B")]),
            com(vec![
                extra("customfield_1", "A"),
                extra("customfield_2", "C"),
            ]),
            com(vec![
                extra("customfield_2", "C"),
                extra("customfield_1", "A"),
            ]),
            cfg.clone(),
        ] {
            let p = plan(&outro, &input(fingerprint(&um)), now);
            assert!(p.reread_all, "{:?}", outro.extra_fields);
        }
    }

    #[test]
    fn limite_fica_com_as_mais_recentes_e_poda_o_resto() {
        let j = setup(true);
        j.fake.set_issues(&tarefas(6));
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert_eq!(numbers(&j.env), vec![1, 2, 3, 4, 5, 6]);
        {
            let l = j.env.sessions.ledger();
            l.ensure_work_item(j.env.project_id, 1, WorkItemKind::Issue)
                .unwrap();
            l.outbox_ack(i64::MAX).unwrap();
        }
        // O limite cai para 3: ficam as 3 mais recentes (6, 5, 4; o falso
        // devolve sem o `--reverse` a ordem inversa) e a 1, com work item.
        let mut cfg = jira_config(&j.fake);
        cfg.max_issues = 3;
        assign(&j.env, cfg, true, 2);
        j.fake.clear_calls();
        let r = j.sync.sync_all().unwrap();
        assert!(r[0].ok, "{r:?}");
        assert_eq!(numbers(&j.env), vec![1, 4, 5, 6]);
        let mut removed = removed_in_outbox(&j.env);
        removed.sort();
        assert_eq!(removed, vec![2, 3]);
        assert!(j.fake.viewed().is_empty(), "nada mudou: nada é relido");
        let lists: Vec<String> = j
            .fake
            .calls()
            .into_iter()
            .filter(|c| c.contains("\tissue\tlist\t"))
            .collect();
        assert!(!lists.is_empty());
        assert!(lists
            .iter()
            .all(|c| !c.contains("--reverse") && c.contains("--paginate\t0:3\t")));
    }

    /// Ambiente de sync com JQL `labels = web` e as issues 1 a 6 já
    /// espelhadas (o cadastro antes era sem JQL); `inside` = as que o JQL
    /// traz agora.
    fn com_jql(inside: &[i64]) -> Jira {
        let j = setup(true);
        let mut issues = tarefas(6);
        // 5 é sub-tarefa de 6 (pai fora do JQL junto com a filha).
        issues[4]["fields"]["parent"] = serde_json::json!({ "key": "PROJ-6" });
        issues[4]["fields"]["issuetype"]["subtask"] = true.into();
        j.fake.set_issues(&issues);
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert_eq!(numbers(&j.env), vec![1, 2, 3, 4, 5, 6]);
        let mut cfg = jira_config(&j.fake);
        cfg.jql = "labels = web".into();
        assign(&j.env, cfg, true, 2);
        let dentro: Vec<Value> = issues
            .iter()
            .filter(|v| inside.contains(&v["key"].as_str().unwrap()[5..].parse::<i64>().unwrap()))
            .cloned()
            .collect();
        j.fake.set_issues(&dentro);
        j.fake.set_list_for("labels = web", Some(&dentro));
        j.env.sessions.ledger().outbox_ack(i64::MAX).unwrap();
        j.fake.clear_calls();
        j
    }

    fn removed_in_outbox(env: &TestEnv) -> Vec<i64> {
        env.sessions
            .ledger()
            .outbox_pending(1000)
            .unwrap()
            .into_iter()
            .filter_map(|e| match e.change {
                SyncChange::IssueMirror(m) if m.removed => Some(m.issue.gh_number),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn mudar_o_jql_poda_o_espelho_sem_reler_tudo() {
        let j = com_jql(&[1, 2]);
        let r = j.sync.sync_all().unwrap();
        assert!(r[0].ok, "{r:?}");
        assert!(j.fake.viewed().is_empty(), "nada mudou: nada é relido");
        assert_eq!(numbers(&j.env), vec![1, 2]);
        let mut removed = removed_in_outbox(&j.env);
        removed.sort();
        assert_eq!(removed, vec![3, 4, 5, 6], "o hub recebe a remoção");
        // A listagem do JQL puro foi feita, além do recorte.
        assert!(j.fake.list_filters().iter().any(|f| f == "labels = web"));
    }

    #[test]
    fn issue_com_work_item_e_o_pai_dela_ficam_e_sao_lidos_pela_chave() {
        let j = com_jql(&[1, 2]);
        {
            let l = j.env.sessions.ledger();
            // Concluído conta: qualquer status.
            let wi = l
                .ensure_work_item(j.env.project_id, 5, WorkItemKind::Issue)
                .unwrap();
            l.set_work_item_status(wi.id, WorkItemStatus::Concluido, "teste")
                .unwrap();
            l.outbox_ack(i64::MAX).unwrap();
        }
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert_eq!(
            numbers(&j.env),
            vec![1, 2, 5, 6],
            "5 tem work item, 6 é o pai"
        );
        let mut removed = removed_in_outbox(&j.env);
        removed.sort();
        assert_eq!(removed, vec![3, 4]);
        let l = j.env.sessions.ledger();
        assert_eq!(
            l.etag(&pinned_key("deep-ion-ai", "demo"))
                .unwrap()
                .as_deref(),
            Some("PROJ-5 PROJ-6")
        );
        drop(l);
        // No ciclo seguinte (incremental), as mantidas são lidas pela chave
        // e atualizadas, mesmo fora do JQL.
        let mut cinco = api_issue("PROJ-5", "Tarefa", "done");
        cinco["fields"]["parent"] = serde_json::json!({ "key": "PROJ-6" });
        cinco["fields"]["issuetype"]["subtask"] = true.into();
        j.fake.set_view(&cinco);
        j.fake.set_view(&api_issue("PROJ-6", "Tarefa", "new"));
        j.fake.clear_calls();
        assert!(j.sync.sync_all().unwrap()[0].ok);
        let mut viewed = j.fake.viewed();
        viewed.sort();
        assert_eq!(viewed, vec!["PROJ-5", "PROJ-6"]);
        let l = j.env.sessions.ledger();
        assert_eq!(
            l.get_issue(j.env.project_id, 5).unwrap().unwrap().state,
            "closed"
        );
    }

    #[test]
    fn listagem_do_jql_falhando_ou_vazia_nao_poda() {
        let j = com_jql(&[1, 2]);
        j.fake.set_list_for("labels = web", None);
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert_eq!(numbers(&j.env), vec![1, 2, 3, 4, 5, 6]);
        assert!(removed_in_outbox(&j.env).is_empty());

        let j = com_jql(&[1, 2]);
        j.fake.set_list_for("labels = web", Some(&[]));
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert_eq!(numbers(&j.env), vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn poda_grande_com_o_mesmo_jql_e_recusada() {
        // O JQL acabou de mudar: a poda de 4 em 6 acontece.
        let j = com_jql(&[1, 2, 3, 4, 5, 6]);
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert_eq!(numbers(&j.env).len(), 6);
        // Na reconciliação seguinte, com o mesmo JQL, a listagem vem só com
        // a 1: tirar 5 de 6 é tido como erro.
        let um: Vec<Value> = tarefas(1);
        j.fake.set_list_for("labels = web", Some(&um));
        let key = full_mark_key("deep-ion-ai", "demo");
        {
            // Marca de mais de um dia: o próximo ciclo é reconciliação.
            let l = j.env.sessions.ledger();
            let mark = l.etag(&key).unwrap().unwrap();
            let (_, fp) = mark.split_once(' ').unwrap();
            l.apply_github(&GithubSnapshot {
                project_id: j.env.project_id,
                etags: vec![(key.clone(), format!("{} {fp}", now_ms() - FULL_EVERY_MS))],
                synced_at: now_ms(),
                ..GithubSnapshot::default()
            })
            .unwrap();
        }
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert_eq!(numbers(&j.env).len(), 6, "sem poda");
        // Com o JQL mudado, a mesma poda grande acontece.
        let mut cfg = jira_config(&j.fake);
        cfg.jql = "labels = outra".into();
        assign(&j.env, cfg, true, 3);
        j.fake.set_list_for("labels = outra", Some(&um));
        assert!(j.sync.sync_all().unwrap()[0].ok);
        assert_eq!(numbers(&j.env), vec![1]);
    }
}
