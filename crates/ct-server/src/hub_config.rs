//! Pull da configuração hub → ambiente (spec do hub como plano de controle
//! §4; issue #177).
//!
//! O ambiente pede `GET <hub_url>/sync/config?since=<config_rev>` com o token
//! de sync e o `X-Ctower-Env-Id`, grava a resposta no `ledger.db` como cache
//! somente leitura e aplica as atribuições: clona ou valida o clone em
//! `repo_path` (vazio = `<repos_root>/<repo>`) e relata o resultado pelo
//! outbox (`project_location` com `state`, `error` e `applied_rev`). Gravar
//! o cache nunca põe fotografia de projeto no outbox; só o relato sobe.
//!
//! - roda no boot, a cada `config_pull_interval_secs` e sob demanda
//!   (`hub.sync.now`, `ctower hub sync`), com o mesmo backoff do envio;
//! - o pull só grava o cache (rápido, limitado pelo timeout do HTTP). A
//!   aplicação (clone, que pode demorar) roda à parte, numa thread
//!   disparada depois do pull, uma por vez: nem o pull seguinte nem o envio
//!   do outbox esperam por um clone;
//! - hub fora do ar: o cache fica como está e continua valendo; passado
//!   `secrets_max_age_hours` desde o último pull bem-sucedido,
//!   `env.info.hub_config.stale` liga e o spawn avisa (#166);
//! - `404` em `/sync/config` = hub anterior ao pull (migração da #182): não
//!   é erro, e os projetos locais legados continuam valendo até o primeiro
//!   pull que responder;
//! - secrets (#184): o pull manda a chave pública do ambiente
//!   (`X-Ctower-Env-Pubkey`, de `<config>/env.key`) e grava a entrega do hub,
//!   quando vem, em `<data>/secrets-cache.age` (veja `crate::env_secrets`).

use crate::checkout::{ensure_checkout, expand_home};
use crate::hub_sync::{status_error, HubSender, MAX_BACKOFF};
use ct_http::sync::{ConfigProject, ConfigReply, ConfigResponse, ENV_ID_HEADER, ENV_PUBKEY_HEADER};
use ct_ledger::{Ledger, LocationState};
use ct_sessions::GithubSync;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Padrão de `config_pull_interval_secs`.
pub const PULL_EVERY: Duration = Duration::from_secs(60);

/// Como o ambiente aplica a configuração.
#[derive(Clone)]
pub struct ConfigOptions {
    /// Raiz dos clones das atribuições sem `repo_path` (`repos_root`).
    pub repos_root: PathBuf,
    /// Intervalo do pull com o hub no ar.
    pub pull_every: Duration,
    /// `gh` e token do GitHub do ambiente para clonar; `None` = `gh` sem token.
    pub github: Option<Arc<GithubSync>>,
}

impl Default for ConfigOptions {
    fn default() -> Self {
        ConfigOptions {
            repos_root: PathBuf::from(expand_home("~/projetos")),
            pull_every: PULL_EVERY,
            github: None,
        }
    }
}

/// Estado do pull para `env.info` (`hub_config`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubConfigStatus {
    /// O ambiente tem `hub_url` e `hub.token`.
    pub linked: bool,
    /// `config_rev` do cache local; `None` = nunca puxou.
    pub config_rev: Option<i64>,
    /// Quando o hub respondeu ao último pull (base da idade do cache).
    pub pulled_at: Option<i64>,
    /// Idade do cache em ms, no momento da resposta.
    pub age_ms: Option<i64>,
    /// Limite de idade (`secrets_max_age_hours`) em ms; `None` = sem aviso.
    pub max_age_ms: Option<i64>,
    /// A configuração (e os secrets do cache) passou do limite de idade: as
    /// sessões novas sobem com o cache, com aviso (#166).
    pub stale: bool,
    pub last_attempt_at: Option<i64>,
    pub last_error: Option<String>,
    /// Falhas seguidas (base do backoff).
    pub failures: u32,
    /// O hub respondeu `404` em `/sync/config`: é anterior ao pull, e os
    /// projetos locais legados continuam valendo.
    pub legacy_hub: bool,
    /// Há token de agente gravado (`hub-agent.token`): o gestor de projetos
    /// pode rodar neste ambiente (spec do gestor §6.1).
    #[serde(default)]
    pub agent_token: bool,
}

/// Resultado de um pull (dentro de `hub.sync.now`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ConfigPullResult {
    /// O hub respondeu (ou é um hub antigo, com `404`).
    pub ok: bool,
    /// A configuração mudou desde o último pull.
    pub changed: bool,
    pub config_rev: Option<i64>,
    /// `owner/repo` que entraram, mudaram ou saíram do cache.
    pub added: Vec<String>,
    pub updated: Vec<String>,
    pub unassigned: Vec<String>,
    /// Atribuições a aplicar (clone ou validação), disparadas em segundo
    /// plano depois do pull; o relato vai ao hub no envio seguinte.
    pub pending_applies: usize,
    pub error: Option<String>,
    pub legacy_hub: bool,
    /// Entradas (secrets e variáveis) recebidas neste pull e gravadas no
    /// cache do ambiente; `None` = o hub não entregou (sem mudança ou hub
    /// anterior à #184), e o cache ficou como estava.
    pub secrets: Option<usize>,
}

/// Estado em memória do pull.
#[derive(Debug, Default)]
pub(crate) struct PullState {
    last_attempt_at: Option<i64>,
    last_error: Option<String>,
    failures: u32,
    legacy_hub: bool,
}

/// `repo_path` da atribuição: vazio = `<repos_root>/<repo>`; `~` expandido.
pub fn resolve_repo_path(repos_root: &Path, p: &ConfigProject) -> String {
    let path = p.repo_path.trim();
    if path.is_empty() {
        repos_root.join(&p.repo).display().to_string()
    } else {
        expand_home(path)
    }
}

/// `hub_config` do `env.info`. Sem envio configurado: `linked = false`, com
/// o que o cache tiver. `max_age`: limite de idade da configuração
/// (`secrets_max_age_hours`).
pub fn status(
    sender: Option<&HubSender>,
    ledger: &Ledger,
    max_age: Option<Duration>,
) -> HubConfigStatus {
    let mut st = match sender {
        Some(s) => {
            let p = s.pull_state();
            HubConfigStatus {
                linked: true,
                last_attempt_at: p.last_attempt_at,
                last_error: p.last_error.clone(),
                failures: p.failures,
                legacy_hub: p.legacy_hub,
                ..Default::default()
            }
        }
        None => HubConfigStatus::default(),
    };
    st.config_rev = ledger.hub_config_rev().ok().flatten();
    st.pulled_at = ledger.hub_config_pulled_at().ok().flatten();
    st.age_ms = st.pulled_at.map(|t| (ct_core::now_ms() - t).max(0));
    st.max_age_ms = max_age.map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
    st.stale = st.linked && matches!((st.age_ms, st.max_age_ms), (Some(a), Some(m)) if a > m);
    st
}

impl HubSender {
    fn pull_state(&self) -> std::sync::MutexGuard<'_, PullState> {
        self.pull.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Puxa a configuração e grava o cache, sem aplicar as atribuições (veja
    /// [`HubSender::apply_assignments`]). Um pull por vez. Síncrono: chame
    /// fora do runtime.
    pub fn pull_config(&self) -> ConfigPullResult {
        let _one = self.pull_lock.lock().unwrap_or_else(|e| e.into_inner());
        let now = ct_core::now_ms();
        let mut out = ConfigPullResult::default();
        match self.fetch_and_cache() {
            Ok(Some((cfg, applied, secrets))) => {
                out.ok = true;
                out.secrets = secrets;
                out.changed = cfg.changed;
                out.config_rev = Some(cfg.config_rev);
                out.added = applied.added;
                out.updated = applied.updated;
                out.unassigned = applied.unassigned;
            }
            Ok(None) => {
                out.ok = true;
                out.legacy_hub = true;
            }
            Err(e) => out.error = Some(e),
        }
        {
            let mut st = self.pull_state();
            st.last_attempt_at = Some(now);
            st.legacy_hub = out.legacy_hub;
            match &out.error {
                None => {
                    st.last_error = None;
                    st.failures = 0;
                }
                Some(e) => {
                    st.last_error = Some(e.clone());
                    st.failures = st.failures.saturating_add(1);
                }
            }
        }
        {
            let l = self.sessions.ledger();
            if out.config_rev.is_none() {
                out.config_rev = l.hub_config_rev().ok().flatten();
            }
            out.pending_applies = l.pending_applies().map(|p| p.len()).unwrap_or(0);
        }
        if !(out.added.is_empty() && out.updated.is_empty() && out.unassigned.is_empty()) {
            self.sessions.notify(
                "projects_changed",
                json!({ "config_rev": out.config_rev, "added": out.added,
                        "updated": out.updated, "unassigned": out.unassigned }),
            );
        }
        out
    }

    /// `Ok(None)` = hub antigo (`404`): nada muda no ledger.
    fn fetch_and_cache(
        &self,
    ) -> Result<Option<(ConfigResponse, ct_ledger::ConfigApplied, Option<usize>)>, String> {
        let (env_id, since, epoch) = {
            let l = self.sessions.ledger();
            let e = |r: ct_ledger::Result<Option<i64>>| r.map_err(|e| e.to_string());
            (
                l.env_id().to_string(),
                e(l.hub_config_rev())?,
                e(l.hub_config_epoch())?,
            )
        };
        let mut url = format!("{}/sync/config", self.target.url.trim_end_matches('/'));
        if let (Some(rev), Some(epoch)) = (since, epoch) {
            url.push_str(&format!("?since={rev}&epoch={epoch}"));
        }
        // Sem a chave do ambiente, o pull segue, só sem entrega de secrets.
        let pubkey = match self.env_cache.public_key() {
            Ok(k) => Some(k.to_string()),
            Err(e) => {
                tracing::warn!("secrets: chave do ambiente indisponível ({e}); pull sem secrets");
                None
            }
        };
        let mut req = self
            .agent
            .get(&url)
            .set("Authorization", &format!("Bearer {}", self.target.token))
            .set(ENV_ID_HEADER, &env_id);
        if let Some(k) = &pubkey {
            req = req.set(ENV_PUBKEY_HEADER, k);
        }
        let reply: ConfigReply = match req.call() {
            Ok(r) => r
                .into_json()
                .map_err(|e| format!("resposta inválida do hub: {e}"))?,
            Err(ureq::Error::Status(404, _)) => return Ok(None),
            Err(ureq::Error::Status(code, r)) => {
                return Err(status_error(code, &r.into_string().unwrap_or_default()))
            }
            Err(ureq::Error::Transport(t)) => return Err(format!("hub inacessível: {t}")),
        };
        // Secrets antes do cache da configuração: se a gravação falhar, o
        // `config_rev` local não avança e o hub entrega de novo no próximo
        // pull. Os erros nunca citam valores.
        let secrets = match &reply.secrets {
            Some(b64) => {
                let s = ct_secrets::decode_delivery(b64)
                    .and_then(|blob| self.env_cache.store(&blob))
                    .map_err(|e| format!("cache de secrets: {e}"))?;
                tracing::info!(
                    "hub: secrets entregues ({} entrada(s), configuração {})",
                    s.entries().len(),
                    s.config_rev()
                );
                Some(s.entries().len())
            }
            None => None,
        };
        let cfg = reply.config;
        let root = self.config.repos_root.clone();
        let applied = self
            .sessions
            .ledger()
            .apply_hub_config(&cfg, |p| resolve_repo_path(&root, p))
            .map_err(|e| e.to_string())?;
        if !applied.is_empty() {
            tracing::info!(
                "hub: configuração {} (entraram {:?}, mudaram {:?}, saíram {:?})",
                cfg.config_rev,
                applied.added,
                applied.updated,
                applied.unassigned
            );
        }
        Ok(Some((cfg, applied, secrets)))
    }

    /// Puxa de `GET /sync/done` os work items `concluido` no hub e troca o
    /// cache do ledger (#298). Hub antigo (`404`) = nada muda.
    pub fn pull_done(&self) -> Result<(), String> {
        let env_id = self.sessions.ledger().env_id().to_string();
        let url = format!("{}/sync/done", self.target.url.trim_end_matches('/'));
        let done: ct_http::sync::DoneResponse = match self
            .agent
            .get(&url)
            .set("Authorization", &format!("Bearer {}", self.target.token))
            .set(ENV_ID_HEADER, &env_id)
            .call()
        {
            Ok(r) => r
                .into_json()
                .map_err(|e| format!("resposta inválida do hub: {e}"))?,
            Err(ureq::Error::Status(404, _)) => return Ok(()),
            Err(ureq::Error::Status(code, r)) => {
                return Err(status_error(code, &r.into_string().unwrap_or_default()))
            }
            Err(ureq::Error::Transport(t)) => return Err(format!("hub inacessível: {t}")),
        };
        self.sessions
            .ledger()
            .apply_hub_done(&done)
            .map_err(|e| e.to_string())
    }

    /// Dispara a aplicação numa thread e volta na hora. Se já há uma
    /// aplicação em andamento, ela pega as pendências na próxima vez.
    pub fn spawn_apply(self: &Arc<Self>) {
        let s = Arc::clone(self);
        std::thread::spawn(move || s.apply_assignments(false));
    }

    /// Clona ou valida o clone das atribuições pendentes (nunca aplicadas,
    /// alteradas ou com erro, inclusive com o hub fora), sem segurar o ledger
    /// nem o pull durante o clone, e relata cada resultado pelo outbox. Uma
    /// aplicação por vez: com `wait = false` e outra em andamento, devolve
    /// `None`; senão, quantos relatos entraram. Síncrono.
    pub fn apply_assignments(&self, wait: bool) -> Option<usize> {
        self.apply_where(wait, false)
    }

    /// Só a validação das atribuições cujo clone já existe no disco (sem
    /// rede, rápida), na hora: o `sync_now` chama antes do envio, para o
    /// relato ir no mesmo lote. Os clones novos ficam para a aplicação em
    /// segundo plano. Com outra aplicação em andamento, `None`.
    pub fn apply_existing(&self) -> Option<usize> {
        self.apply_where(false, true)
    }

    fn apply_where(&self, wait: bool, only_existing: bool) -> Option<usize> {
        let _one = if wait {
            self.apply_lock.lock().unwrap_or_else(|e| e.into_inner())
        } else {
            match self.apply_lock.try_lock() {
                Ok(g) => g,
                Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => return None,
            }
        };
        let reports = self.apply_pending(only_existing);
        if reports > 0 {
            self.sessions
                .notify("projects_changed", json!({ "reports": reports }));
        }
        Some(reports)
    }

    fn apply_pending(&self, only_existing: bool) -> usize {
        let pending = match self.sessions.ledger().pending_applies() {
            Ok(p) if only_existing => p
                .into_iter()
                .filter(|p| Path::new(&p.repo_path).exists())
                .collect(),
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("hub: atribuições pendentes ilegíveis: {e}");
                return 0;
            }
        };
        let (gh_bin, token) = match &self.config.github {
            Some(g) => (
                g.client().config().gh_bin.clone(),
                g.client().configured_token(),
            ),
            None => ("gh".to_string(), None),
        };
        let mut reports = 0;
        for p in pending {
            let Some(rev) = p.config_rev else { continue };
            // `_projeto/` nasce ao aplicar a atribuição (#440). Sem
            // `docs_root`, com docs desligados ou inacessível, nada (aviso).
            self.sessions.ensure_project_docs(&p);
            let r = ensure_checkout(
                &p.gh_owner,
                &p.gh_repo,
                &p.forge,
                &p.repo_path,
                &gh_bin,
                token.as_deref(),
            );
            let (state, error) = match &r {
                Ok(()) => (LocationState::Ok, None),
                Err(e) => {
                    tracing::warn!(
                        "hub: atribuição de {}/{} em {} falhou: {e}",
                        p.gh_owner,
                        p.gh_repo,
                        p.repo_path
                    );
                    (LocationState::Error, Some(e.to_string()))
                }
            };
            match self
                .sessions
                .ledger()
                .record_apply(p.id, rev, state, error.as_deref())
            {
                Ok(true) => reports += 1,
                Ok(false) => {}
                Err(e) => tracing::warn!("hub: relato de {}/{} falhou: {e}", p.gh_owner, p.gh_repo),
            }
        }
        reports
    }

    /// Espera até o próximo pull: `pull_every` com o hub no ar, dobrando a
    /// cada falha seguida até `MAX_BACKOFF` (ou `pull_every`, se maior).
    pub fn next_pull_delay(&self) -> Duration {
        let failures = self.pull_state().failures;
        let every = self.config.pull_every;
        every
            .checked_mul(1u32 << failures.min(16))
            .unwrap_or(MAX_BACKOFF)
            .min(MAX_BACKOFF.max(every))
    }
}

/// Laço do pull no daemon: no boot e depois a cada `pull_every`, com backoff.
/// Cada pull dispara a aplicação em segundo plano.
pub fn spawn_puller(sender: Arc<HubSender>) {
    tokio::spawn(async move {
        loop {
            let s = Arc::clone(&sender);
            match tokio::task::spawn_blocking(move || s.pull_config()).await {
                Ok(r) => {
                    if let Some(e) = &r.error {
                        tracing::warn!("hub: pull da configuração falhou: {e}");
                    } else if r.legacy_hub {
                        tracing::debug!(
                            "hub sem /sync/config (anterior ao pull): projetos locais mantidos"
                        );
                    }
                }
                Err(e) => tracing::warn!("hub: pull da configuração falhou: {e}"),
            }
            let s = Arc::clone(&sender);
            match tokio::task::spawn_blocking(move || s.pull_done()).await {
                Ok(Err(e)) => tracing::warn!("hub: pull dos concluídos falhou: {e}"),
                Err(e) => tracing::warn!("hub: pull dos concluídos falhou: {e}"),
                Ok(Ok(())) => {}
            }
            sender.spawn_apply();
            tokio::time::sleep(sender.next_pull_delay()).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub_sync::{HubSyncResult, HubTarget};
    use crate::test_util::{serve_hub, Srv};
    use ct_ledger::{AssignmentFilter, AssignmentState, Hub, HubProjectPatch, SharedHub};
    use ct_sessions::test_support::{git_in, FakeClaude};

    const DEMO: (&str, &str) = ("deep-ion-ai", "demo");

    fn new_hub() -> SharedHub {
        let hub = Hub::open_in_memory().unwrap();
        for repo in ["demo", "outro"] {
            hub.add_project(&ct_ledger::NewHubProject {
                owner: "deep-ion-ai".into(),
                repo: repo.into(),
                name: repo.into(),
                base_branch: "develop".into(),
                protected_branches: vec!["develop".into(), "main".into()],
            })
            .unwrap();
        }
        SharedHub::new(hub)
    }

    /// `cthubd` de verdade no listener (preso até aqui, sem corrida de porta).
    async fn hub_up(hub: &SharedHub) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        serve_hub(hub.clone(), listener).await;
        url
    }

    /// Ambiente com o clone do projeto de teste apontando para o GitHub, e
    /// um `gh` que falha (nenhum teste clona da rede).
    fn env() -> Srv {
        let srv = Srv::new(FakeClaude::exit(0));
        git_in(
            &srv.env.repo,
            &[
                "remote",
                "add",
                "origin",
                "git@github.com:deep-ion-ai/demo.git",
            ],
        );
        srv
    }

    fn sender(srv: &Srv, url: &str, token: &str) -> Arc<HubSender> {
        sender_with(srv, url, token, "echo 'gh: sem rede no teste' >&2\nexit 1")
    }

    /// `gh` falso com o corpo `script` (nenhum teste clona da rede).
    fn sender_with(srv: &Srv, url: &str, token: &str, script: &str) -> Arc<HubSender> {
        let bin = srv.env.tmp.path().join("gh-falso");
        std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let client = ct_gh::GhClient::new(
            ct_gh::GhConfig::new("http://127.0.0.1:1", bin.to_str().unwrap())
                .with_token_timeout(ct_gh::fake::FAKE_GH_TOKEN_TIMEOUT)
                .with_token_file(srv.env.tmp.path().join("github.token")),
        );
        let github = Arc::new(GithubSync::new(srv.env.sessions.clone(), client));
        Arc::new(
            HubSender::new(
                srv.env.sessions.clone(),
                HubTarget {
                    url: url.to_string(),
                    token: token.to_string(),
                },
                Duration::from_secs(5),
            )
            .with_config(ConfigOptions {
                repos_root: srv.env.tmp.path().join("projetos"),
                pull_every: Duration::from_secs(60),
                github: Some(github),
            }),
        )
    }

    async fn pull(s: &Arc<HubSender>) -> ConfigPullResult {
        let s = Arc::clone(s);
        tokio::task::spawn_blocking(move || s.pull_config())
            .await
            .unwrap()
    }

    /// Aplicação das atribuições, esperando a que estiver em andamento.
    async fn apply(s: &Arc<HubSender>) -> usize {
        let s = Arc::clone(s);
        tokio::task::spawn_blocking(move || s.apply_assignments(true).unwrap())
            .await
            .unwrap()
    }

    async fn sync_now(s: &Arc<HubSender>) -> HubSyncResult {
        let s = Arc::clone(s);
        tokio::task::spawn_blocking(move || s.sync_now())
            .await
            .unwrap()
    }

    fn estado(hub: &SharedHub, env_id: &str) -> Vec<(String, AssignmentState, Option<String>)> {
        hub.lock()
            .list_assignments(&AssignmentFilter {
                env: Some(env_id.into()),
                ..Default::default()
            })
            .unwrap()
            .into_iter()
            .map(|a| (a.gh_repo, a.state, a.error))
            .collect()
    }

    fn repos(srv: &Srv) -> Vec<String> {
        srv.env
            .sessions
            .ledger()
            .list_projects()
            .unwrap()
            .into_iter()
            .map(|p| p.gh_repo)
            .collect()
    }

    /// Docs do projeto (#440): o modelo vem do hub pelo pull e `_projeto/`
    /// nasce ao aplicar a atribuição; mudar o modelo no hub reaplica e cria
    /// as pastas novas, sem mexer no que já existe.
    #[tokio::test(flavor = "multi_thread")]
    async fn aplicar_atribuicao_cria_a_pasta_de_docs_do_projeto() {
        let hub = new_hub();
        let docs = |dirs: &[&str]| HubProjectPatch {
            docs: Some(ct_core::DocsConfig {
                enabled: true,
                project_dirs: dirs.iter().map(|d| d.to_string()).collect(),
                epic_dirs: vec!["prompts".into()],
            }),
            ..Default::default()
        };
        hub.lock()
            .update_project(DEMO.0, DEMO.1, &docs(&["analises"]))
            .unwrap();
        let tok = hub.lock().add_legacy_env_token("a").unwrap();
        let url = hub_up(&hub).await;
        let a = env();
        let env_a = a.env.sessions.ledger().env_id().to_string();
        let sa = sender(&a, &url, &tok);
        assert!(sync_now(&sa).await.error.is_none());
        let repo_a = a.env.repo.display().to_string();
        hub.lock()
            .add_assignment(DEMO.0, DEMO.1, &env_a, Some(&repo_a))
            .unwrap();
        let root = a.env.docs_root_up();

        assert!(pull(&sa).await.ok);
        let p = a.env.sessions.ledger().list_projects().unwrap().remove(0);
        assert!(p.docs.enabled);
        assert_eq!(apply(&sa).await, 1);
        let base = root.join("deep-ion-ai__demo/_projeto");
        assert!(base.join("analises").is_dir());
        std::fs::write(base.join("analises/nota.md"), "minha").unwrap();

        hub.lock()
            .update_project(DEMO.0, DEMO.1, &docs(&["analises", "specs"]))
            .unwrap();
        assert!(pull(&sa).await.ok);
        apply(&sa).await;
        assert!(base.join("specs").is_dir());
        assert_eq!(
            std::fs::read_to_string(base.join("analises/nota.md")).unwrap(),
            "minha"
        );
    }

    /// O pull só traz os projetos atribuídos, a atribuição aplicada continua
    /// aplicada depois de pulls e envios repetidos (e do resync), e mudar a
    /// `base_branch` no hub chega sem reiniciar.
    #[tokio::test(flavor = "multi_thread")]
    async fn pull_so_dos_atribuidos_aplica_e_relata() {
        let hub = new_hub();
        let (tok_a, tok_b) = {
            let h = hub.lock();
            (
                h.add_legacy_env_token("a").unwrap(),
                h.add_legacy_env_token("b").unwrap(),
            )
        };
        let url = hub_up(&hub).await;
        let a = env();
        let b = env();
        let (env_a, env_b) = (
            a.env.sessions.ledger().env_id().to_string(),
            b.env.sessions.ledger().env_id().to_string(),
        );
        let sa = sender(&a, &url, &tok_a);
        let sb = sender(&b, &url, &tok_b);
        // Primeiro envio: o hub conhece os dois ambientes.
        assert!(sync_now(&sa).await.error.is_none());
        assert!(sync_now(&sb).await.error.is_none());
        let repo_a = a.env.repo.display().to_string();
        {
            let h = hub.lock();
            h.add_assignment(DEMO.0, DEMO.1, &env_a, Some(&repo_a))
                .unwrap();
            // `repo_path` vazio: `<repos_root>/outro`, que não existe (clona).
            h.add_assignment(DEMO.0, "outro", &env_b, None).unwrap();
        }
        assert_eq!(estado(&hub, &env_a)[0].1, AssignmentState::Pending);

        let r = sync_now(&sa).await;
        assert!(r.ok, "{r:?}");
        let c = r.config.unwrap();
        assert!(c.ok && c.changed && c.error.is_none(), "{c:?}");
        assert_eq!(c.updated, vec!["deep-ion-ai/demo"]);
        assert_eq!(c.pending_applies, 1);
        // A aplicação roda à parte; o relato sobe no envio seguinte.
        apply(&sa).await;
        assert!(sync_now(&sa).await.ok);
        // Só o atribuído a este ambiente; o legado casado mantém o id.
        assert_eq!(repos(&a), vec!["demo"]);
        let p = a
            .env
            .sessions
            .ledger()
            .get_project(a.env.project_id)
            .unwrap();
        // O `rev` da linha é o da atribuição; o do cache, o do hub.
        let hub_rev = hub.lock().config_rev().unwrap();
        assert!(p.config_rev.unwrap() < hub_rev);
        assert_eq!(
            a.env.sessions.ledger().hub_config_rev().unwrap(),
            Some(hub_rev)
        );
        assert_eq!(p.location_state, Some(LocationState::Ok));
        assert_eq!(estado(&hub, &env_a)[0].1, AssignmentState::Applied);

        // Pulls e envios repetidos, resync: continua aplicada.
        for _ in 0..3 {
            let r = sync_now(&sa).await;
            assert!(r.ok, "{r:?}");
            let c = r.config.unwrap();
            assert!(!c.changed && c.pending_applies == 0, "{c:?}");
            assert_eq!(estado(&hub, &env_a)[0].1, AssignmentState::Applied);
        }
        a.env.sessions.ledger().enqueue_all().unwrap();
        assert!(sync_now(&sa).await.ok);
        assert_eq!(estado(&hub, &env_a)[0].1, AssignmentState::Applied);
        let day = Some(Duration::from_secs(24 * 3600));
        let st = status(Some(&sa), &a.env.sessions.ledger(), day);
        assert!(st.linked && st.last_error.is_none() && !st.legacy_hub);
        assert_eq!(st.config_rev, Some(hub_rev));
        assert!(st.pulled_at.is_some() && st.age_ms.is_some());
        assert_eq!((st.max_age_ms, st.stale), (Some(86_400_000), false));
        // Passou do limite (#166): `stale` liga; sem limite, nunca.
        std::thread::sleep(Duration::from_millis(5));
        let st = status(
            Some(&sa),
            &a.env.sessions.ledger(),
            Some(Duration::from_millis(1)),
        );
        assert!(st.stale, "{st:?}");
        assert!(!status(Some(&sa), &a.env.sessions.ledger(), None).stale);

        // `base_branch` mudou no hub: chega no próximo pull, sem reiniciar.
        hub.lock()
            .update_project(
                DEMO.0,
                DEMO.1,
                &HubProjectPatch {
                    base_branch: Some("main".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        let novo_rev = hub.lock().config_rev().unwrap();
        let c = pull(&sa).await;
        assert!(c.changed && c.pending_applies == 1, "{c:?}");
        assert_eq!(apply(&sa).await, 1);
        let p = a
            .env
            .sessions
            .ledger()
            .get_project(a.env.project_id)
            .unwrap();
        assert_eq!(
            (p.base_branch.as_str(), p.config_rev),
            ("main", Some(novo_rev))
        );
        assert!(sync_now(&sa).await.ok);
        // O relato traz a revisão do projeto aplicada, e a atribuição segue aplicada.
        let a_hub = &hub
            .lock()
            .list_assignments(&AssignmentFilter::default())
            .unwrap()[0];
        assert_eq!(
            (a_hub.state, a_hub.applied_rev),
            (AssignmentState::Applied, Some(novo_rev))
        );

        // O ambiente B só vê o dele; o clone falhou e o erro chega ao hub.
        let r = sync_now(&sb).await;
        assert!(r.ok, "o erro do clone não é erro do sync: {r:?}");
        assert_eq!(repos(&b), vec!["outro"]);
        apply(&sb).await;
        assert!(sync_now(&sb).await.ok);
        let e = estado(&hub, &env_b);
        assert_eq!(e[0].1, AssignmentState::Error, "{e:?}");
        let msg = e[0].2.as_deref().unwrap();
        assert!(msg.contains("sem rede no teste"), "{e:?}");
        let destino = b.env.tmp.path().join("projetos").join("outro");
        assert!(msg.contains(&destino.display().to_string()), "{msg}");
        // Erro de novo no próximo pull: tenta outra vez, sem relato repetido.
        let c = pull(&sb).await;
        assert_eq!(c.pending_applies, 1, "{c:?}");
        assert_eq!(apply(&sb).await, 0);
        // O legado de B (demo), não atribuído a ele, saiu da lista.
        let fora = b.env.sessions.ledger().list_unassigned_projects().unwrap();
        assert_eq!(fora[0].gh_repo, "demo");
        assert!(b.env.repo.exists());
    }

    /// Um clone lento não segura o envio do outbox nem o pull seguinte.
    #[tokio::test(flavor = "multi_thread")]
    async fn sync_now_nao_espera_o_clone() {
        let hub = new_hub();
        let tok = hub.lock().add_legacy_env_token("a").unwrap();
        let url = hub_up(&hub).await;
        let a = env();
        let env_a = a.env.sessions.ledger().env_id().to_string();
        let sa = sender_with(&a, &url, &tok, "sleep 3\necho 'gh: lento' >&2\nexit 1");
        assert!(sync_now(&sa).await.error.is_none());
        hub.lock()
            .add_assignment(DEMO.0, "outro", &env_a, None)
            .unwrap();
        a.env
            .sessions
            .ledger()
            .ensure_work_item(a.env.project_id, 11, ct_core::WorkItemKind::Issue)
            .unwrap();
        let t = std::time::Instant::now();
        let r = sync_now(&sa).await;
        assert!(r.ok && r.sent > 0, "{r:?}");
        assert_eq!(r.config.as_ref().unwrap().pending_applies, 1);
        // Outro pull durante o clone também não espera.
        assert!(pull(&sa).await.ok);
        assert!(t.elapsed() < Duration::from_secs(2), "{:?}", t.elapsed());
        // O clone termina (com erro) e o relato chega no envio seguinte.
        apply(&sa).await;
        assert!(sync_now(&sa).await.ok);
        let e = estado(&hub, &env_a);
        assert_eq!(e[0].1, AssignmentState::Error, "{e:?}");
        assert!(e[0].2.as_deref().unwrap().contains("gh: lento"));
    }

    /// `repos_root` mudou no ambiente, sem mudança no hub: o caminho novo é
    /// clonado/validado e relatado.
    #[tokio::test(flavor = "multi_thread")]
    async fn repos_root_novo_reaplica() {
        let hub = new_hub();
        let tok = hub.lock().add_legacy_env_token("a").unwrap();
        let url = hub_up(&hub).await;
        let a = env();
        let env_a = a.env.sessions.ledger().env_id().to_string();
        // Clones já existentes em duas raízes.
        let root = |n: &str| a.env.tmp.path().join(n);
        for r in ["raiz1", "raiz2"] {
            let d = root(r).join("outro");
            std::fs::create_dir_all(&d).unwrap();
            git_in(&d, &["init", "-q"]);
            git_in(
                &d,
                &[
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/deep-ion-ai/outro",
                ],
            );
        }
        let com_raiz = |r: &str| {
            let s = sender(&a, &url, &tok);
            let mut s = Arc::into_inner(s).unwrap();
            s.config.repos_root = root(r);
            Arc::new(s)
        };
        let s1 = com_raiz("raiz1");
        assert!(sync_now(&s1).await.error.is_none());
        hub.lock()
            .add_assignment(DEMO.0, "outro", &env_a, None)
            .unwrap();
        sync_now(&s1).await;
        apply(&s1).await;
        assert!(sync_now(&s1).await.ok);
        let rel = || {
            hub.lock()
                .list_assignments(&AssignmentFilter::default())
                .unwrap()[0]
                .clone()
        };
        assert_eq!(rel().state, AssignmentState::Applied);
        let path1 = root("raiz1").join("outro").display().to_string();
        assert_eq!(rel().reported_path.as_deref(), Some(path1.as_str()));

        // Daemon reiniciado com outra raiz: o hub responde `changed: false`.
        let s2 = com_raiz("raiz2");
        let c = pull(&s2).await;
        assert!(!c.changed, "{c:?}");
        assert_eq!((c.updated.len(), c.pending_applies), (1, 1), "{c:?}");
        assert_eq!(apply(&s2).await, 1);
        assert!(sync_now(&s2).await.ok);
        let path2 = root("raiz2").join("outro").display().to_string();
        assert_eq!(rel().reported_path.as_deref(), Some(path2.as_str()));
        assert_eq!(rel().state, AssignmentState::Applied);
    }

    /// A remoção chega por `removed` e pela ausência em `projects`; o clone
    /// fica no disco.
    #[tokio::test(flavor = "multi_thread")]
    async fn remocao_chega_e_o_clone_fica() {
        let hub = new_hub();
        let tok = hub.lock().add_legacy_env_token("a").unwrap();
        let url = hub_up(&hub).await;
        let a = env();
        let env_a = a.env.sessions.ledger().env_id().to_string();
        let sa = sender(&a, &url, &tok);
        assert!(sync_now(&sa).await.error.is_none());
        let repo_a = a.env.repo.display().to_string();
        hub.lock()
            .add_assignment(DEMO.0, DEMO.1, &env_a, Some(&repo_a))
            .unwrap();
        assert!(sync_now(&sa).await.ok);
        assert_eq!(repos(&a), vec!["demo"]);

        // Por `removed`: a atribuição removida depois do `since`.
        hub.lock()
            .remove_assignment(DEMO.0, DEMO.1, &env_a)
            .unwrap();
        let (since, epoch) = {
            let l = a.env.sessions.ledger();
            (l.hub_config_rev().unwrap(), l.hub_config_epoch().unwrap())
        };
        let cfg = hub.lock().env_config(&env_a, since, epoch).unwrap();
        assert_eq!(cfg.removed.as_ref().unwrap()[0].repo, "demo");
        let c = pull(&sa).await;
        assert_eq!(c.unassigned, vec!["deep-ion-ai/demo"], "{c:?}");
        assert!(repos(&a).is_empty());
        let fora = a.env.sessions.ledger().list_unassigned_projects().unwrap();
        assert_eq!(fora[0].id, a.env.project_id);
        assert!(a.env.repo.join(".git").exists(), "o clone fica no disco");

        // Pela ausência em `projects`: um cache com projeto que o hub não
        // manda mais (ex.: hub restaurado de snapshot).
        hub.lock()
            .add_assignment(DEMO.0, DEMO.1, &env_a, Some(&repo_a))
            .unwrap();
        pull(&sa).await;
        assert_eq!(repos(&a), vec!["demo"]);
        hub.lock()
            .conn()
            .execute("DELETE FROM assignment", [])
            .unwrap();
        hub.lock()
            .conn()
            .execute(
                "UPDATE hub_meta SET value = value + 1 WHERE key = 'config_rev'",
                [],
            )
            .unwrap();
        let c = pull(&sa).await;
        assert_eq!(c.unassigned, vec!["deep-ion-ai/demo"], "{c:?}");
        assert!(repos(&a).is_empty());
    }

    /// Hub fora do ar: o cache fica, sem limite de idade, e o erro aparece
    /// no `hub_config`.
    #[tokio::test(flavor = "multi_thread")]
    async fn hub_fora_mantem_o_cache() {
        let hub = new_hub();
        let tok = hub.lock().add_legacy_env_token("a").unwrap();
        let url = hub_up(&hub).await;
        let a = env();
        let env_a = a.env.sessions.ledger().env_id().to_string();
        let up = sender(&a, &url, &tok);
        assert!(sync_now(&up).await.error.is_none());
        let repo_a = a.env.repo.display().to_string();
        hub.lock()
            .add_assignment(DEMO.0, DEMO.1, &env_a, Some(&repo_a))
            .unwrap();
        assert!(pull(&up).await.ok);
        let rev = a.env.sessions.ledger().hub_config_rev().unwrap();

        let dead = crate::test_util::DeadHub::new();
        let down = sender(&a, &dead.url, &tok);
        for _ in 0..2 {
            let c = pull(&down).await;
            assert!(!c.ok, "{c:?}");
            assert!(
                c.error.as_deref().unwrap().contains("hub inacessível"),
                "{c:?}"
            );
        }
        assert_eq!(repos(&a), vec!["demo"]);
        assert_eq!(a.env.sessions.ledger().hub_config_rev().unwrap(), rev);
        let st = status(Some(&down), &a.env.sessions.ledger(), None);
        assert_eq!(st.failures, 2);
        assert!(st.last_error.unwrap().contains("hub inacessível"));
        assert_eq!(st.config_rev, rev);
        assert_eq!(down.next_pull_delay(), Duration::from_secs(240));
        let r = sync_now(&down).await;
        assert!(!r.ok && r.config.unwrap().error.is_some());
    }

    /// Hub antigo (sem `/sync/config`): `404` não é erro, e os projetos
    /// locais legados continuam valendo.
    #[tokio::test(flavor = "multi_thread")]
    async fn hub_antigo_404_mantem_os_legados() {
        let app = axum::Router::new().route(
            "/sync/push",
            axum::routing::post(|| async { axum::Json(serde_json::json!({ "last_seq": 0 })) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let a = env();
        let s = sender(&a, &url, "t");
        let c = pull(&s).await;
        assert!(c.ok && c.legacy_hub && c.error.is_none(), "{c:?}");
        assert_eq!(repos(&a), vec!["demo"]);
        let p = a
            .env
            .sessions
            .ledger()
            .get_project(a.env.project_id)
            .unwrap();
        assert_eq!((p.config_rev, p.unassigned_at), (None, None));
        assert_eq!(a.env.sessions.ledger().hub_config_rev().unwrap(), None);
        let st = status(Some(&s), &a.env.sessions.ledger(), None);
        assert!(st.legacy_hub && st.last_error.is_none() && st.failures == 0);
        let r = sync_now(&s).await;
        assert!(r.config.unwrap().legacy_hub);
    }

    #[test]
    fn caminho_padrao_e_repos_root() {
        let p = ConfigProject {
            docs: Default::default(),
            owner: "o".into(),
            repo: "r".into(),
            name: "r".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
            repo_path: String::new(),
            rev: 1,
            tracker: Default::default(),
            forge: Default::default(),
            branch_pattern: None,
            attribution: true,
            sync: false,
            rules: Default::default(),
        };
        assert_eq!(resolve_repo_path(Path::new("/src"), &p), "/src/r");
        let abs = ConfigProject {
            repo_path: "/x/r".into(),
            ..p.clone()
        };
        assert_eq!(resolve_repo_path(Path::new("/src"), &abs), "/x/r");
        let home = ConfigProject {
            repo_path: "~/r".into(),
            ..p
        };
        assert!(!resolve_repo_path(Path::new("/src"), &home).starts_with('~'));
    }
}
