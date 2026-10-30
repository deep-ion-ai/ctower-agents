//! Secrets e variáveis entregues pelo hub ao ambiente (issue #184, épico
//! #171). O pull da configuração (`hub_config`) manda a chave pública do
//! ambiente e grava a entrega no cache; aqui ficam o acesso ao cache e a
//! rotação da chave (`ctower secrets rotate-env-key`).
//!
//! - chave: `<config>/env.key` (age X25519, 0600), gerada no primeiro pull;
//! - cache: `<data>/secrets-cache.age` (0600), o arquivo age da última
//!   entrega como veio do hub, com o `config_rev`. Hub fora do ar: vale o
//!   último cache; passado `secrets_max_age_hours`, o spawn avisa (#166);
//! - o "registro" do ambiente é só o `cthubd add-env` no hub (token): a chave
//!   pública vai ao hub no pull, que roda no boot do `ctowerd`.
//!
//! A injeção no spawn e no resume (#166) fica no `ct-sessions`
//! (`Sessions::session_secrets`), que lê o mesmo cache e resolve por projeto
//! com `EnvSecrets::resolve(Some("owner/repo"))` (projeto sobrepõe global).

use crate::hub_config::ConfigPullResult;
use crate::hub_sync::HubSender;
use crate::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use ct_secrets::{EnvCache, EnvSecrets, SecretsError};
use ct_sessions::Dirs;
use serde::{Deserialize, Serialize};

/// Chave e cache de secrets do ambiente em `dirs`.
pub fn env_cache(dirs: &Dirs) -> EnvCache {
    EnvCache::new(dirs.env_key_file(), dirs.secrets_cache_file())
}

/// Secrets entregues pelo hub, do cache local (funciona com o hub fora do
/// ar). `None` = nenhuma entrega ainda.
pub fn load(dirs: &Dirs) -> Result<Option<EnvSecrets>, SecretsError> {
    env_cache(dirs).load()
}

/// Resposta de `POST /secrets/rotate-env-key`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotateEnvKeyResult {
    /// Chave pública nova (`age1...`).
    pub public_key: String,
    /// O cache foi recifrado para a chave nova (continua usável offline).
    pub cache_kept: bool,
    /// Pull feito logo depois, que manda a chave nova ao hub (e recebe a
    /// reentrega). `None` = ambiente não ligado a um hub.
    pub pull: Option<ConfigPullResult>,
}

impl HubSender {
    /// Troca a chave do ambiente (sem pull no meio) e puxa a configuração:
    /// a chave pública nova faz o hub reentregar os secrets. Síncrono.
    pub fn rotate_env_key(&self) -> Result<RotateEnvKeyResult, String> {
        let rotated = {
            let _one = self.pull_lock.lock().unwrap_or_else(|e| e.into_inner());
            self.env_cache.rotate_key().map_err(|e| e.to_string())?
        };
        tracing::info!("secrets: chave do ambiente trocada ({})", rotated.public);
        let pull = self.pull_config();
        Ok(RotateEnvKeyResult {
            public_key: rotated.public.to_string(),
            cache_kept: rotated.cache_kept,
            pull: Some(pull),
        })
    }
}

/// Rotação sem hub ligado: só a chave e o cache locais.
fn rotate_local(cache: &EnvCache) -> Result<RotateEnvKeyResult, String> {
    let r = cache.rotate_key().map_err(|e| e.to_string())?;
    Ok(RotateEnvKeyResult {
        public_key: r.public.to_string(),
        cache_kept: r.cache_kept,
        pull: None,
    })
}

/// `POST /secrets/rotate-env-key` (token de UI): `ctower secrets
/// rotate-env-key`.
pub(crate) async fn rotate_env_key_http(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if !crate::bearer_ok(&headers, &st.ui_token) {
        return crate::unauthorized();
    }
    let r = match st.hub_sender.clone() {
        Some(s) => tokio::task::spawn_blocking(move || s.rotate_env_key()).await,
        None => {
            let cache = env_cache(&st.sessions.config().dirs);
            tokio::task::spawn_blocking(move || rotate_local(&cache)).await
        }
    };
    match r {
        Ok(Ok(r)) => axum::Json(r).into_response(),
        Ok(Err(e)) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub_sync::HubTarget;
    use crate::test_util::{serve_hub_state, DeadHub, Srv, UI};
    use ct_ledger::{Hub, SharedHub};
    use ct_secrets::{Identity, Kind, Scope, SecretValue};
    use ct_sessions::test_support::FakeClaude;
    use std::sync::Arc;
    use std::time::Duration;

    const DEMO: &str = "deep-ion-ai/demo";

    fn sender(srv: &Srv, url: &str, token: &str) -> Arc<HubSender> {
        Arc::new(HubSender::new(
            srv.env.sessions.clone(),
            HubTarget {
                url: url.to_string(),
                token: token.to_string(),
            },
            Duration::from_secs(5),
        ))
    }

    async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        tokio::task::spawn_blocking(f).await.unwrap()
    }

    /// `(escopo, nome, valor)` do cache do ambiente.
    fn cache(srv: &Srv) -> Vec<(String, String, String)> {
        let s = load(&srv.env.sessions.config().dirs)
            .unwrap()
            .expect("sem cache");
        s.entries()
            .iter()
            .map(|e| {
                let scope = match &e.scope {
                    Scope::Global => "global".to_string(),
                    Scope::Project(k) => k.clone(),
                };
                (scope, e.name.clone(), e.value.expose().to_string())
            })
            .collect()
    }

    fn g() -> (String, String, String) {
        ("global".into(), "G".into(), "g".into())
    }

    /// Ponta a ponta com um `cthubd` de verdade: o ambiente recebe só os
    /// globais e os do projeto atribuído; o cache só abre com a chave dele;
    /// desatribuir tira os do projeto; hub fora mantém o cache; a rotação
    /// mantém o cache legível e o hub reentrega; revogar corta a entrega.
    #[tokio::test(flavor = "multi_thread")]
    async fn entrega_ponta_a_ponta() {
        let d = tempfile::tempdir().unwrap();
        let hub_dirs = cthubd::config::Dirs {
            config: d.path().join("config"),
            data: d.path().join("data"),
        };
        hub_dirs.ensure().unwrap();
        let hub = SharedHub::new(Hub::open_in_memory().unwrap());
        let tok = {
            let h = hub.lock();
            for repo in ["demo", "outro"] {
                h.add_project(&ct_ledger::NewHubProject {
                    owner: "deep-ion-ai".into(),
                    repo: repo.into(),
                    name: repo.into(),
                    base_branch: "develop".into(),
                    protected_branches: vec![],
                })
                .unwrap();
            }
            h.add_legacy_env_token("a").unwrap()
        };
        let secrets = cthubd::secrets::HubSecrets::open(&hub_dirs, hub.clone()).unwrap();
        secrets
            .set(Scope::Global, "G", Kind::Variable, SecretValue::new("g"))
            .unwrap();
        secrets
            .set(
                Scope::parse_project(DEMO).unwrap(),
                "D",
                Kind::Secret,
                SecretValue::new("d-secreto"),
            )
            .unwrap();
        secrets
            .set(
                Scope::parse_project("deep-ion-ai/outro").unwrap(),
                "O",
                Kind::Secret,
                SecretValue::new("o-secreto"),
            )
            .unwrap();
        let mut st = cthubd::HubState::new(hub.clone(), "ui-do-hub", "hub", "hub-id");
        st.secrets = Some(Arc::new(secrets));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        serve_hub_state(st, listener).await;

        let a = Srv::new(FakeClaude::exit(0));
        let dirs = a.env.sessions.config().dirs.clone();
        let env_a = a.env.sessions.ledger().env_id().to_string();
        let up = sender(&a, &url, &tok);
        assert!(load(&dirs).unwrap().is_none());
        // Primeiro pull: chave gerada e enviada; nada atribuído = só globais.
        let s = Arc::clone(&up);
        let r = blocking(move || s.pull_config()).await;
        assert!(r.ok, "{r:?}");
        assert_eq!(r.secrets, Some(1));
        assert_eq!(cache(&a), [g()]);
        let mode = |p: &std::path::Path| {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(p).unwrap().permissions().mode() & 0o777
        };
        assert_eq!(mode(&dirs.env_key_file()), 0o600);
        assert_eq!(mode(&dirs.secrets_cache_file()), 0o600);
        assert!(hub.lock().env_pubkey(&env_a).unwrap().is_some());

        hub.lock()
            .add_assignment(
                "deep-ion-ai",
                "demo",
                &env_a,
                Some(&a.env.repo.display().to_string()),
            )
            .unwrap();
        let s = Arc::clone(&up);
        let r = blocking(move || s.pull_config()).await;
        assert_eq!(r.secrets, Some(2), "{r:?}");
        assert_eq!(
            cache(&a),
            [g(), (DEMO.into(), "D".into(), "d-secreto".into())]
        );
        let bruto = std::fs::read(dirs.secrets_cache_file()).unwrap();
        assert!(!String::from_utf8_lossy(&bruto).contains("d-secreto"));
        assert!(!String::from_utf8_lossy(&bruto).contains("o-secreto"));
        // Só a chave do ambiente abre o cache.
        assert!(EnvSecrets::open(&bruto, &Identity::generate()).is_err());
        // Resolução por projeto (o que a injeção no spawn usa).
        let l = load(&dirs).unwrap().unwrap();
        let r = l.resolve(Some("Deep-Ion-AI/Demo"));
        assert_eq!(r.keys().collect::<Vec<_>>(), ["D", "G"]);
        assert!(!l.resolve(Some("deep-ion-ai/outro")).contains_key("O"));

        // Em dia: o hub não reentrega e o cache fica.
        let s = Arc::clone(&up);
        let r = blocking(move || s.pull_config()).await;
        assert_eq!((r.changed, r.secrets), (false, None));
        assert_eq!(cache(&a).len(), 2);

        // Desatribuir tira do cache os secrets do projeto.
        hub.lock()
            .remove_assignment("deep-ion-ai", "demo", &env_a)
            .unwrap();
        let s = Arc::clone(&up);
        let r = blocking(move || s.pull_config()).await;
        assert_eq!(r.secrets, Some(1));
        assert_eq!(cache(&a), [g()]);

        // Hub fora: o pull falha e o cache continua utilizável.
        let dead = DeadHub::new();
        let down = sender(&a, &dead.url, &tok);
        let s = Arc::clone(&down);
        let r = blocking(move || s.pull_config()).await;
        assert!(!r.ok && r.secrets.is_none());
        assert_eq!(cache(&a), [g()]);

        // Rotação offline mantém o cache; com o hub no ar, ele reentrega.
        let antiga = ct_secrets::read_identity(&dirs.env_key_file()).unwrap();
        let s = Arc::clone(&down);
        let r = blocking(move || s.rotate_env_key()).await.unwrap();
        assert!(r.cache_kept && !r.pull.unwrap().ok);
        assert_eq!(cache(&a), [g()]);
        let bruto = std::fs::read(dirs.secrets_cache_file()).unwrap();
        assert!(EnvSecrets::open(&bruto, &antiga).is_err());
        let s = Arc::clone(&up);
        let r = blocking(move || s.pull_config()).await;
        assert_eq!((r.changed, r.secrets), (true, Some(1)), "{r:?}");
        let s = Arc::clone(&up);
        let r = blocking(move || s.rotate_env_key()).await.unwrap();
        assert_eq!(r.pull.as_ref().unwrap().secrets, Some(1), "{r:?}");
        assert_eq!(
            hub.lock().env_pubkey(&env_a).unwrap().unwrap().pubkey,
            r.public_key
        );
        assert_eq!(cache(&a), [g()]);

        // Revogado: 401, nada entregue, o cache fica para uso offline.
        assert!(hub.lock().revoke_env_token("a").unwrap());
        let s = Arc::clone(&up);
        let r = blocking(move || s.pull_config()).await;
        assert!(!r.ok && r.secrets.is_none(), "{r:?}");
        assert!(hub.lock().env_pubkey(&env_a).unwrap().is_none());
        assert_eq!(cache(&a), [g()]);
    }

    /// Perda da máquina do hub (#170), ponta a ponta: secrets globais e de
    /// projeto atribuído com a recuperação configurada; o ambiente recebe;
    /// o config_dir do hub some inteiro; `secrets restore` do snapshot com a
    /// chave de recuperação; os valores voltam, o hub ganha chave nova, a
    /// recuperação fica desconfigurada, a lista a rotacionar aparece, o
    /// config_rev sobe e o ambiente recebe tudo de novo.
    #[tokio::test(flavor = "multi_thread")]
    async fn restauracao_com_a_recuperacao_reentrega_aos_ambientes() {
        use cthubd::secrets::restore::{apply_restore, open_snapshot, OpenedWith};
        let d = tempfile::tempdir().unwrap();
        let hub_dirs = cthubd::config::Dirs {
            config: d.path().join("config"),
            data: d.path().join("data"),
        };
        hub_dirs.ensure().unwrap();
        let snaps = d.path().join("OneDrive/cthub");
        let hub = SharedHub::new(Hub::open(&hub_dirs.hub_db()).unwrap());
        let tok = {
            let h = hub.lock();
            h.add_project(&ct_ledger::NewHubProject {
                owner: "deep-ion-ai".into(),
                repo: "demo".into(),
                name: "demo".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
            h.add_legacy_env_token("a").unwrap()
        };
        let secrets = cthubd::secrets::HubSecrets::open(&hub_dirs, hub.clone())
            .unwrap()
            .with_snapshot_dir(snaps.clone());
        let g_rec = secrets.recovery_generate().unwrap();
        secrets.recovery_verify(&g_rec.private_key).unwrap();
        secrets
            .set(Scope::Global, "G", Kind::Variable, SecretValue::new("g"))
            .unwrap();
        secrets
            .set(
                Scope::Global,
                "API_TOKEN",
                Kind::Secret,
                SecretValue::new("token-global-123"),
            )
            .unwrap();
        secrets
            .set(
                Scope::parse_project(DEMO).unwrap(),
                "D",
                Kind::Secret,
                SecretValue::new("d-secreto"),
            )
            .unwrap();
        let antiga = ct_secrets::read_identity(&hub_dirs.secrets_key_file()).unwrap();
        let mut st = cthubd::HubState::new(hub.clone(), "ui-do-hub", "hub", "hub-id");
        st.secrets = Some(Arc::new(secrets));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        serve_hub_state(st, listener).await;

        let a = Srv::new(FakeClaude::exit(0));
        let env_a = a.env.sessions.ledger().env_id().to_string();
        let s = sender(&a, &url, &tok);
        assert!(blocking(move || s.pull_config()).await.ok);
        hub.lock()
            .add_assignment(
                "deep-ion-ai",
                "demo",
                &env_a,
                Some(&a.env.repo.display().to_string()),
            )
            .unwrap();
        let s = sender(&a, &url, &tok);
        let r = blocking(move || s.pull_config()).await;
        assert_eq!(r.secrets, Some(3), "{r:?}");
        let esperado = vec![
            (
                "global".to_string(),
                "API_TOKEN".to_string(),
                "token-global-123".to_string(),
            ),
            g(),
            (DEMO.into(), "D".into(), "d-secreto".into()),
        ];
        assert_eq!(cache(&a), esperado);
        let rev_antes = hub.lock().config_rev().unwrap();
        // O snapshot mais recente na pasta sincronizada é o store atual.
        let store = std::fs::read(hub_dirs.secrets_file()).unwrap();
        let snap = std::fs::read_dir(&snaps)
            .unwrap()
            .map(|e| e.unwrap().path())
            .find(|p| std::fs::read(p).unwrap() == store)
            .unwrap();

        // A máquina do hub se perde: o config_dir some inteiro (o hub.db
        // volta à parte, com `cthubd restore`; aqui ele fica).
        std::fs::remove_dir_all(&hub_dirs.config).unwrap();
        let rec = ct_secrets::Zeroizing::new(g_rec.private_key.clone());
        let opened = open_snapshot(&hub_dirs, &snap, || Ok(rec.clone())).unwrap();
        assert_eq!(opened.opened_with(), OpenedWith::RecoveryKey);
        let rep = apply_restore(&hub_dirs, opened, &snaps).unwrap();
        assert!(rep.config_rev > rev_antes);
        assert_eq!(rep.entries, 3);
        let nova = ct_secrets::read_identity(&hub_dirs.secrets_key_file()).unwrap();
        assert_ne!(nova.to_public(), antiga.to_public());
        let names: Vec<_> = rep.rotate_pending.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["API_TOKEN", "D"]);

        // O serviço sobe de novo com o store restaurado.
        let hub2 = SharedHub::new(Hub::open(&hub_dirs.hub_db()).unwrap());
        let secrets2 = cthubd::secrets::HubSecrets::open(&hub_dirs, hub2.clone()).unwrap();
        assert!(!secrets2.recovery_status().configured);
        assert_eq!(
            secrets2.resolve(Some(DEMO))["D"].value.expose(),
            "d-secreto"
        );
        assert_eq!(
            secrets2.resolve(None)["API_TOKEN"].value.expose(),
            "token-global-123"
        );
        assert_eq!(
            secrets2.list().unwrap().rotate_pending.unwrap().items.len(),
            2
        );
        let mut st2 = cthubd::HubState::new(hub2.clone(), "ui-do-hub", "hub", "hub-id");
        st2.secrets = Some(Arc::new(secrets2));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url2 = format!("http://{}", listener.local_addr().unwrap());
        serve_hub_state(st2, listener).await;

        // O ambiente recebe a reentrega com os mesmos valores.
        let s = sender(&a, &url2, &tok);
        let r = blocking(move || s.pull_config()).await;
        assert!(r.ok && r.changed, "{r:?}");
        assert_eq!(r.secrets, Some(3), "{r:?}");
        assert_eq!(cache(&a), esperado);
        let s = sender(&a, &url2, &tok);
        let r = blocking(move || s.pull_config()).await;
        assert_eq!((r.changed, r.secrets), (false, None));
    }

    /// `POST /secrets/rotate-env-key`: token de UI; sem hub, só local.
    #[tokio::test(flavor = "multi_thread")]
    async fn rota_de_rotacao() {
        let srv = Srv::new(FakeClaude::exit(0));
        let dirs = srv.env.sessions.config().dirs.clone();
        let antes = env_cache(&dirs).public_key().unwrap();
        let (s, _) = srv
            .post(
                "/secrets/rotate-env-key",
                Some("x"),
                &[],
                serde_json::json!({}),
            )
            .await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, v) = srv
            .post(
                "/secrets/rotate-env-key",
                Some(UI),
                &[],
                serde_json::json!({}),
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert!(v["pull"].is_null());
        assert_eq!(v["cache_kept"], false);
        let depois = env_cache(&dirs).public_key().unwrap();
        assert_ne!(antes, depois);
        assert_eq!(v["public_key"], depois.to_string());
    }
}
