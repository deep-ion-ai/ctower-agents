//! Atualização do ambiente (issue #90): versão instalada e disponível, com
//! verificação sob demanda (`env.check_update`) e periódica, e o disparo do
//! atualizador (`env.update`). Cada mudança sai no push `update.status`.

use ct_sessions::update::{
    read_state, resolve_bin, running, spawn_runner, systemd_run_bin, Available, Build, Source,
    UpdateState,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast;

/// Estado da atualização deste ambiente (`env.info.update`).
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct UpdateStatus {
    pub installed: Build,
    /// Ref seguido (`update_ref`).
    #[serde(rename = "ref")]
    #[cfg_attr(feature = "ts", ts(rename = "ref"))]
    pub git_ref: String,
    /// Última verificação bem-sucedida; `None` = ainda não verificou.
    pub available: Option<Available>,
    /// Erro da última verificação (some na próxima que der certo).
    pub check_error: Option<String>,
    /// Atualização em andamento ou a última que terminou (`state.json`).
    pub state: Option<UpdateState>,
}

/// Erro de `env.update` com sessões vivas e sem `force`.
pub const LIVE_SESSIONS: i64 = -32010;

/// Por quanto tempo, no máximo, acompanhar o `state.json` depois do disparo.
const WATCH_FOR: Duration = Duration::from_secs(2 * 3600);

pub struct Updates {
    /// `git_ref` muda com `env.config.set update_ref` (#328).
    source: Mutex<Source>,
    /// `ctower` disparado na unidade do atualizador (caminho absoluto).
    ctower: PathBuf,
    systemd_run: String,
    status: Mutex<UpdateStatus>,
    /// Uma verificação por vez (o `git fetch` não gosta de concorrência).
    checking: tokio::sync::Mutex<()>,
    events: broadcast::Sender<Value>,
}

impl Updates {
    pub fn new(source: Source, installed: Build, events: broadcast::Sender<Value>) -> Self {
        let status = UpdateStatus {
            installed,
            git_ref: source.git_ref.clone(),
            available: None,
            check_error: None,
            state: None,
        };
        Updates {
            source: Mutex::new(source),
            ctower: resolve_bin("ctower"),
            systemd_run: systemd_run_bin(),
            status: Mutex::new(status),
            checking: tokio::sync::Mutex::new(()),
            events,
        }
    }

    pub fn source(&self) -> Source {
        self.source.lock().unwrap().clone()
    }

    /// Troca o ref seguido (`update_ref`): a próxima verificação e o
    /// próximo `ctower update --run` (que relê o `config.toml`) usam o novo.
    /// O `available` do ref antigo deixa de valer.
    pub fn set_ref(&self, git_ref: &str) {
        self.source.lock().unwrap().git_ref = git_ref.to_string();
        let st = {
            let mut st = self.status.lock().unwrap();
            if st.git_ref != git_ref {
                st.git_ref = git_ref.to_string();
                st.available = None;
                st.check_error = None;
            }
            st.clone()
        };
        let mut st = st;
        st.state = read_state(&self.dir());
        self.push(&st);
    }

    fn dir(&self) -> PathBuf {
        self.source.lock().unwrap().dir.clone()
    }

    /// `ctower` a disparar (padrão: o ao lado do ctowerd ou o do `PATH`).
    pub fn with_ctower(mut self, ctower: PathBuf) -> Self {
        self.ctower = ctower;
        self
    }

    /// `systemd-run` a usar (testes trocam por um falso).
    pub fn with_systemd_run(mut self, bin: &str) -> Self {
        self.systemd_run = bin.into();
        self
    }

    pub fn status(&self) -> UpdateStatus {
        let mut st = self.status.lock().unwrap().clone();
        st.state = read_state(&self.dir());
        st
    }

    fn push(&self, st: &UpdateStatus) {
        let _ = self
            .events
            .send(json!({ "event": "update.status", "data": st }));
    }

    /// Dispara o atualizador (ou, com `rollback`, a volta à versão anterior,
    /// `ctower update --run --rollback`) e acompanha o `state.json` enquanto
    /// este daemon viver (o restart o encerra; o novo lê o resultado no
    /// boot). Quem chama confere as sessões vivas.
    pub async fn start(self: &Arc<Self>, rollback: bool) -> Result<UpdateStatus, String> {
        let t0 = ct_core::now_ms();
        if running(&self.dir(), t0).is_some() {
            return Err("uma atualização já está em andamento".into());
        }
        let (run, ctower) = (self.systemd_run.clone(), self.ctower.clone());
        tokio::task::spawn_blocking(move || spawn_runner(&run, &ctower, rollback))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        let me = Arc::clone(self);
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + WATCH_FOR;
            let mut last: Option<UpdateState> = None;
            while tokio::time::Instant::now() < deadline {
                let cur = read_state(&me.dir()).filter(|s| s.started_at >= t0);
                if cur.is_some() && cur != last {
                    me.push(&me.status());
                    if cur.as_ref().is_some_and(|s| s.phase.is_terminal()) {
                        break;
                    }
                    last = cur;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        });
        Ok(self.status())
    }

    /// Busca o ref remoto e compara com o instalado. Falha vira
    /// `check_error`, sem apagar o último `available`.
    pub async fn check(&self) -> UpdateStatus {
        let _guard = self.checking.lock().await;
        let (source, installed) = (self.source(), self.status().installed);
        let r = tokio::task::spawn_blocking(move || source.check(&installed, ct_core::now_ms()))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));
        {
            let mut st = self.status.lock().unwrap();
            match r {
                Ok(a) => {
                    st.available = Some(a);
                    st.check_error = None;
                }
                Err(e) => st.check_error = Some(e),
            }
        }
        let st = self.status();
        self.push(&st);
        st
    }
}

/// Verifica logo após o boot e depois a cada `every`.
pub fn spawn_periodic(updates: Arc<Updates>, every: Duration) {
    tokio::spawn(async move {
        loop {
            let st = updates.check().await;
            match (&st.available, &st.check_error) {
                (_, Some(e)) => tracing::warn!("atualização: verificação falhou: {e}"),
                (Some(a), None) if a.update_available => tracing::info!(
                    "atualização disponível em {}: {} ({})",
                    a.git_ref,
                    a.build.version,
                    &a.build.commit[..a.build.commit.len().min(12)]
                ),
                _ => {}
            }
            tokio::time::sleep(every).await;
        }
    });
}
