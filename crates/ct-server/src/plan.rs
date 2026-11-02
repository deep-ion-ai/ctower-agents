//! Plano da conta deste ambiente (issue #106): a conta (`claude auth
//! status`, em cache), a saúde do statusline e a leitura mais recente das
//! sessões, com a regra do reset.

use ct_core::plan::{
    plan_view, EnvPlanReport, PlanAccount, PlanReading, PlanView, StatuslineHealth,
};
use ct_sessions::usage::{
    claude_config_dir, latest_plan, read_account, statusline_health, usage_dir,
};
use ct_sessions::Sessions;
use serde::Serialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Por quanto tempo a conta lida vale antes de rodar `claude auth status`
/// de novo (pega a troca de login).
pub const ACCOUNT_TTL: Duration = Duration::from_secs(300);

static ACCOUNT: Mutex<Option<(Instant, String, Option<PlanAccount>)>> = Mutex::new(None);

/// Conta do ambiente, relida a cada [`ACCOUNT_TTL`]. `None` = conta
/// desconhecida.
pub fn account(claude_bin: &str) -> Option<PlanAccount> {
    {
        let cur = ACCOUNT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, bin, a)) = &*cur {
            if bin == claude_bin && at.elapsed() < ACCOUNT_TTL {
                return a.clone();
            }
        }
    }
    // Fora do lock: o `claude auth status` pode demorar.
    let a = read_account(claude_bin);
    *ACCOUNT.lock().unwrap_or_else(|e| e.into_inner()) =
        Some((Instant::now(), claude_bin.to_string(), a.clone()));
    a
}

/// Esquece a conta lida (depois de instalar o statusline ou de um login).
pub fn forget_account() {
    *ACCOUNT.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

/// Saúde do statusline deste ambiente.
pub fn statusline(sessions: &Sessions) -> StatuslineHealth {
    statusline_health(claude_config_dir().as_deref(), &sessions.config().dirs)
}

/// O que vai ao hub no envio.
pub fn report(sessions: &Sessions, now: i64) -> EnvPlanReport {
    EnvPlanReport {
        account: account(&sessions.config().claude_bin),
        statusline: statusline(sessions),
        latest: latest_plan(&usage_dir(&sessions.config().dirs), now),
    }
}

/// Resposta de `account.get`: a conta do ambiente e o plano dela **pelas
/// sessões deste ambiente** (o hub junta os ambientes da mesma conta em
/// `hub.plan.accounts`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EnvAccount {
    /// `None` = conta desconhecida (sem login ou `claude auth status`
    /// falhou): fica fora de qualquer agregado.
    pub account: Option<PlanAccount>,
    pub statusline: StatuslineHealth,
    /// Com a regra do reset aplicada em `now`.
    pub plan: PlanView,
    /// Leitura bruta mais recente (por janela), para a UI reaplicar a regra
    /// com o relógio dela.
    pub latest: Option<PlanReading>,
    /// Relógio do ambiente na resposta (ms).
    pub now: i64,
}

pub fn env_account(sessions: &Sessions, now: i64) -> EnvAccount {
    let r = report(sessions, now);
    EnvAccount {
        plan: plan_view(r.latest.as_ref(), now),
        account: r.account,
        statusline: r.statusline,
        latest: r.latest,
        now,
    }
}
