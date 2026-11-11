//! Uso do plano do Claude Code por conta (issue #106): a leitura que o
//! statusline grava, a conta do ambiente e o agregador com a regra do reset.
//!
//! O percentual das janelas (5h e semanal) é da **conta**, não do projeto
//! nem do ambiente. Vários ambientes podem usar a mesma conta; a leitura
//! mais recente de qualquer um deles vale para todos. Leituras de contas
//! diferentes nunca se misturam.

use serde::{Deserialize, Serialize};

/// Versão do registro do statusline no `settings.json` do Claude Code. Sobe
/// quando o comando registrado muda de formato; ambiente com versão menor
/// aparece como desatualizado.
pub const STATUSLINE_VERSION: u32 = 1;

/// Uma janela do plano numa leitura.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlanWindow {
    /// 0 a 100 (pode passar de 100).
    pub used_percentage: f64,
    /// Quando a janela renova (ms).
    pub resets_at: i64,
}

/// Leitura do plano: as janelas que o Claude Code entregou ao statusline
/// numa atualização de sessão. Cada janela pode faltar.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlanReading {
    /// Quando o statusline gravou a leitura (ms).
    pub read_at: i64,
    #[serde(default)]
    pub five_hour: Option<PlanWindow>,
    #[serde(default)]
    pub seven_day: Option<PlanWindow>,
}

impl PlanReading {
    /// Leitura dos `rate_limits` do JSON do statusline (`resets_at` em
    /// segundos). `None` sem nenhuma das duas janelas.
    pub fn from_rate_limits(rate_limits: &serde_json::Value, read_at: i64) -> Option<Self> {
        let window = |k: &str| {
            let w = rate_limits.get(k)?;
            Some(PlanWindow {
                used_percentage: w.get("used_percentage")?.as_f64()?,
                resets_at: w.get("resets_at")?.as_i64()?.checked_mul(1000)?,
            })
        };
        let r = PlanReading {
            read_at,
            five_hour: window("five_hour"),
            seven_day: window("seven_day"),
        };
        (r.five_hour.is_some() || r.seven_day.is_some()).then_some(r)
    }
}

/// Conta do Claude Code de um ambiente (`claude auth status`). O e-mail e a
/// organização só aparecem na UI; nunca vão para logs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlanAccount {
    /// Chave de agrupamento: `orgId` com `email` ([`account_key`]).
    pub key: String,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub org_name: Option<String>,
    /// `pro`, `max`… (`subscriptionType`).
    #[serde(default)]
    pub subscription_type: Option<String>,
}

/// Chave da conta: `orgId` com `email`, só `orgId` se o e-mail faltar.
/// `None` sem `orgId` (conta desconhecida).
pub fn account_key(org_id: Option<&str>, email: Option<&str>) -> Option<String> {
    let org = org_id.map(str::trim).filter(|s| !s.is_empty())?;
    Some(match email.map(str::trim).filter(|s| !s.is_empty()) {
        Some(e) => format!("{org}/{}", e.to_lowercase()),
        None => org.to_string(),
    })
}

/// Variável com o `orgId` da conta, para o ambiente com login por token
/// (`CLAUDE_CODE_OAUTH_TOKEN`), em que o `claude auth status` não traz a conta.
pub const DECLARED_ORG_ID_ENV: &str = "CTOWER_CLAUDE_ORG_ID";
/// Variável com o e-mail da conta (opcional, mas sem ele a chave não junta
/// com a dos ambientes que têm o e-mail no `claude auth status`).
pub const DECLARED_EMAIL_ENV: &str = "CTOWER_CLAUDE_EMAIL";

impl PlanAccount {
    /// Conta declarada pelo operador ([`DECLARED_ORG_ID_ENV`] e
    /// [`DECLARED_EMAIL_ENV`]), com a mesma chave que o `claude auth status`
    /// daria para ela. `None` sem `orgId`.
    pub fn declared(org_id: Option<&str>, email: Option<&str>) -> Option<Self> {
        let email = email
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from);
        Some(PlanAccount {
            key: account_key(org_id, email.as_deref())?,
            email,
            org_name: None,
            subscription_type: None,
        })
    }

    /// Conta declarada pelas variáveis de ambiente do processo.
    pub fn declared_from_env() -> Option<Self> {
        let var = |k: &str| std::env::var(k).ok();
        Self::declared(
            var(DECLARED_ORG_ID_ENV).as_deref(),
            var(DECLARED_EMAIL_ENV).as_deref(),
        )
    }

    /// Conta da saída JSON de `claude auth status`. `None` sem login ou sem
    /// `orgId` ("conta desconhecida").
    pub fn from_auth_status(v: &serde_json::Value) -> Option<Self> {
        Self::from_auth_status_or(v, None)
    }

    /// Como [`Self::from_auth_status`], mas com login e sem `orgId` na saída
    /// (login por token, `authMethod: "oauth_token"`) vale a conta
    /// `declared`. Com o `orgId` na saída, ela vence a declarada. Sem login,
    /// `None` mesmo com a declarada: não há leituras dessa conta aqui.
    pub fn from_auth_status_or(v: &serde_json::Value, declared: Option<&Self>) -> Option<Self> {
        if v.get("loggedIn").and_then(serde_json::Value::as_bool) != Some(true) {
            return None;
        }
        let s = |k: &str| {
            v.get(k)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from)
        };
        let email = s("email");
        let Some(key) = account_key(s("orgId").as_deref(), email.as_deref()) else {
            return declared.cloned().map(|d| PlanAccount {
                subscription_type: d.subscription_type.or_else(|| s("subscriptionType")),
                ..d
            });
        };
        Some(PlanAccount {
            key,
            email,
            org_name: s("orgName"),
            subscription_type: s("subscriptionType"),
        })
    }
}

/// Saúde do statusline no ambiente.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct StatuslineHealth {
    /// O nosso statusline está registrado no `settings.json` do Claude Code.
    pub installed: bool,
    /// Versão registrada; `None` = não instalado.
    #[serde(default)]
    pub version: Option<u32>,
    /// Versão que este ambiente instala ([`STATUSLINE_VERSION`]).
    pub current: u32,
    /// Havia um statusline de terceiros, executado dentro do nosso.
    #[serde(default)]
    pub chained: bool,
    /// Por que não deu para ler o estado (pasta de config ausente, JSON
    /// inválido…).
    #[serde(default)]
    pub error: Option<String>,
}

impl StatuslineHealth {
    /// Instalado numa versão anterior: a UI oferece atualizar.
    pub fn outdated(&self) -> bool {
        self.installed && self.version.unwrap_or(0) < self.current
    }
}

/// O que o ambiente informa ao hub sobre o plano (vai em `POST /sync/push`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct EnvPlanReport {
    /// `None` = conta desconhecida (sem login ou `claude auth status` falhou).
    #[serde(default)]
    pub account: Option<PlanAccount>,
    #[serde(default)]
    pub statusline: StatuslineHealth,
    /// Leitura mais recente de cada janela entre as sessões do ambiente.
    #[serde(default)]
    pub latest: Option<PlanReading>,
}

/// Uma janela depois da regra do reset.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WindowView {
    /// A leitura vale (`agora < resets_at`).
    Ok {
        used_percentage: f64,
        resets_at: i64,
        read_at: i64,
    },
    /// A janela já renovou desde a leitura (`agora >= resets_at`): o
    /// percentual antigo não vale, e o próximo reset é desconhecido até
    /// chegar leitura nova.
    Renewed { resets_at: i64, read_at: i64 },
    /// Nenhuma sessão gravou esta janela.
    None,
}

/// As duas janelas de uma conta.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct PlanView {
    pub five_hour: WindowView,
    pub seven_day: WindowView,
    /// Leitura mais recente usada (ms); `None` = sem leitura.
    pub read_at: Option<i64>,
}

fn view_of(w: Latest, now: i64) -> WindowView {
    match w {
        None => WindowView::None,
        Some((w, read_at)) if now < w.resets_at => WindowView::Ok {
            used_percentage: w.used_percentage,
            resets_at: w.resets_at,
            read_at,
        },
        Some((w, read_at)) => WindowView::Renewed {
            resets_at: w.resets_at,
            read_at,
        },
    }
}

type Latest = Option<(PlanWindow, i64)>;

/// Por janela, a leitura mais recente que a traz (por `read_at`, não pelo
/// maior percentual), com o `read_at` dela.
fn latest_windows<'a>(readings: impl IntoIterator<Item = &'a PlanReading>) -> (Latest, Latest) {
    let pick = |cur: &mut Latest, w: Option<PlanWindow>, at: i64| {
        if let Some(w) = w {
            if cur.is_none_or(|(_, t)| at > t) {
                *cur = Some((w, at));
            }
        }
    };
    let (mut five, mut seven) = (None, None);
    for r in readings {
        pick(&mut five, r.five_hour, r.read_at);
        pick(&mut seven, r.seven_day, r.read_at);
    }
    (five, seven)
}

fn newest(five: Latest, seven: Latest) -> Option<i64> {
    five.map(|(_, t)| t).max(seven.map(|(_, t)| t))
}

/// Junta leituras **da mesma conta** numa só: por janela, a mais recente.
/// `None` sem nenhuma janela.
pub fn merge_readings<'a>(
    readings: impl IntoIterator<Item = &'a PlanReading>,
) -> Option<PlanReading> {
    let (five, seven) = latest_windows(readings);
    Some(PlanReading {
        read_at: newest(five, seven)?,
        five_hour: five.map(|(w, _)| w),
        seven_day: seven.map(|(w, _)| w),
    })
}

/// Visão do plano de uma conta em `now` (ms), com a regra do reset.
/// `readings` são as leituras **de uma só conta**.
pub fn plan_view<'a>(readings: impl IntoIterator<Item = &'a PlanReading>, now: i64) -> PlanView {
    let (five, seven) = latest_windows(readings);
    PlanView {
        five_hour: view_of(five, now),
        seven_day: view_of(seven, now),
        read_at: newest(five, seven),
    }
}

/// Um ambiente numa conta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AccountEnv {
    pub env_id: String,
    pub name: String,
    pub statusline: StatuslineHealth,
}

/// Plano de uma conta e os ambientes que a usam (`hub.plan.accounts`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AccountPlan {
    pub account: PlanAccount,
    pub plan: PlanView,
    pub envs: Vec<AccountEnv>,
}

/// Contas do hub: agrupa os ambientes pela chave da conta e aplica a regra
/// do reset. Devolve as contas (ordenadas pelo rótulo) e os ambientes de
/// conta desconhecida (fora de qualquer agregado).
pub fn group_accounts(
    envs: impl IntoIterator<Item = (String, String, EnvPlanReport)>,
    now: i64,
) -> (Vec<AccountPlan>, Vec<AccountEnv>) {
    let mut by_key: std::collections::BTreeMap<
        String,
        (PlanAccount, Vec<PlanReading>, Vec<AccountEnv>),
    > = Default::default();
    let mut unknown = Vec::new();
    for (env_id, name, report) in envs {
        let env = AccountEnv {
            env_id,
            name,
            statusline: report.statusline,
        };
        let Some(account) = report.account else {
            unknown.push(env);
            continue;
        };
        let entry = by_key
            .entry(account.key.clone())
            .or_insert_with(|| (account.clone(), Vec::new(), Vec::new()));
        // Rótulos (e-mail, organização) mais recentes valem: o primeiro com
        // valor fica.
        if entry.0.email.is_none() {
            entry.0.email = account.email;
        }
        if entry.0.org_name.is_none() {
            entry.0.org_name = account.org_name;
        }
        if entry.0.subscription_type.is_none() {
            entry.0.subscription_type = account.subscription_type;
        }
        entry.1.extend(report.latest);
        entry.2.push(env);
    }
    let mut accounts: Vec<AccountPlan> = by_key
        .into_values()
        .map(|(account, readings, mut envs)| {
            envs.sort_by(|a, b| a.name.cmp(&b.name));
            AccountPlan {
                plan: plan_view(&readings, now),
                account,
                envs,
            }
        })
        .collect();
    accounts.sort_by_key(|a| account_label(&a.account));
    unknown.sort_by(|a, b| a.name.cmp(&b.name));
    (accounts, unknown)
}

fn account_label(a: &PlanAccount) -> String {
    a.email
        .clone()
        .or_else(|| a.org_name.clone())
        .unwrap_or_else(|| a.key.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const H: i64 = 3_600_000;

    fn reading(read_at: i64, five: Option<(f64, i64)>, seven: Option<(f64, i64)>) -> PlanReading {
        let w = |(p, r): (f64, i64)| PlanWindow {
            used_percentage: p,
            resets_at: r,
        };
        PlanReading {
            read_at,
            five_hour: five.map(w),
            seven_day: seven.map(w),
        }
    }

    fn account(key: &str) -> PlanAccount {
        PlanAccount {
            key: key.into(),
            email: Some(format!("{key}@example.com")),
            org_name: None,
            subscription_type: Some("max".into()),
        }
    }

    #[test]
    fn leitura_dos_rate_limits_converte_segundos() {
        let r = PlanReading::from_rate_limits(
            &json!({ "five_hour": { "used_percentage": 23.5, "resets_at": 1000 } }),
            5,
        )
        .unwrap();
        assert_eq!(
            r.five_hour,
            Some(PlanWindow {
                used_percentage: 23.5,
                resets_at: 1_000_000
            })
        );
        assert_eq!(r.seven_day, None);
        assert_eq!(PlanReading::from_rate_limits(&json!({}), 5), None);
        assert_eq!(PlanReading::from_rate_limits(&json!(null), 5), None);
    }

    #[test]
    fn chave_da_conta() {
        assert_eq!(
            account_key(Some("conta-a"), Some("Fulano@Example.com")).as_deref(),
            Some("conta-a/fulano@example.com")
        );
        assert_eq!(
            account_key(Some("conta-a"), None).as_deref(),
            Some("conta-a")
        );
        assert_eq!(
            account_key(Some("conta-a"), Some(" ")).as_deref(),
            Some("conta-a")
        );
        assert_eq!(account_key(None, Some("fulano@example.com")), None);
    }

    #[test]
    fn conta_do_auth_status() {
        let a = PlanAccount::from_auth_status(&json!({
            "loggedIn": true, "orgId": "conta-a", "email": "fulano@example.com",
            "orgName": "Acme", "subscriptionType": "max"
        }))
        .unwrap();
        assert_eq!(a.key, "conta-a/fulano@example.com");
        assert_eq!(a.org_name.as_deref(), Some("Acme"));
        assert_eq!(
            PlanAccount::from_auth_status(&json!({ "loggedIn": false, "orgId": "conta-a" })),
            None
        );
        assert_eq!(
            PlanAccount::from_auth_status(&json!({ "loggedIn": true })),
            None
        );
    }

    #[test]
    fn conta_declarada_tem_a_mesma_chave_do_auth_status() {
        let d = PlanAccount::declared(Some(" conta-a "), Some("Fulano@Example.com")).unwrap();
        let a = PlanAccount::from_auth_status(&json!({
            "loggedIn": true, "orgId": "conta-a", "email": "fulano@example.com"
        }))
        .unwrap();
        assert_eq!(d.key, a.key);
        assert_eq!(d.email.as_deref(), Some("Fulano@Example.com"));
        assert_eq!(
            PlanAccount::declared(Some("conta-a"), Some(" "))
                .unwrap()
                .key,
            "conta-a"
        );
        assert_eq!(
            PlanAccount::declared(None, Some("fulano@example.com")),
            None
        );
        assert_eq!(PlanAccount::declared(Some(" "), None), None);
    }

    #[test]
    fn login_por_token_usa_a_conta_declarada() {
        let declared = PlanAccount::declared(Some("conta-a"), Some("fulano@example.com"));
        // Saída do `claude auth status` com CLAUDE_CODE_OAUTH_TOKEN: sem conta.
        let token = json!({
            "loggedIn": true, "authMethod": "oauth_token", "apiProvider": "firstParty"
        });
        assert_eq!(PlanAccount::from_auth_status(&token), None);
        let a = PlanAccount::from_auth_status_or(&token, declared.as_ref()).unwrap();
        assert_eq!(a.key, "conta-a/fulano@example.com");
        assert_eq!(a.subscription_type, None);
        // Sem login, nem a declarada vale.
        assert_eq!(
            PlanAccount::from_auth_status_or(&json!({ "loggedIn": false }), declared.as_ref()),
            None
        );
        // Com a conta na saída, ela vence a declarada.
        let b = PlanAccount::from_auth_status_or(
            &json!({ "loggedIn": true, "orgId": "conta-b", "email": "beltrano@example.com",
                     "subscriptionType": "max" }),
            declared.as_ref(),
        )
        .unwrap();
        assert_eq!(b.key, "conta-b/beltrano@example.com");
        // A declarada pega o plano da saída, se vier.
        let c = PlanAccount::from_auth_status_or(
            &json!({ "loggedIn": true, "subscriptionType": "max" }),
            declared.as_ref(),
        )
        .unwrap();
        assert_eq!(c.subscription_type.as_deref(), Some("max"));
    }

    #[test]
    fn antes_do_reset_o_percentual_vale() {
        let now = 10 * H;
        let v = plan_view(
            &[reading(9 * H, Some((72.0, 12 * H)), Some((64.0, 100 * H)))],
            now,
        );
        assert_eq!(
            v.five_hour,
            WindowView::Ok {
                used_percentage: 72.0,
                resets_at: 12 * H,
                read_at: 9 * H
            }
        );
        assert!(
            matches!(v.seven_day, WindowView::Ok { used_percentage, .. } if used_percentage == 64.0)
        );
        assert_eq!(v.read_at, Some(9 * H));
    }

    #[test]
    fn depois_do_reset_nao_devolve_o_percentual_antigo() {
        let r = [reading(9 * H, Some((99.0, 12 * H)), Some((64.0, 100 * H)))];
        // Exatamente no reset já conta como renovada.
        for now in [12 * H, 13 * H] {
            let v = plan_view(&r, now);
            assert_eq!(
                v.five_hour,
                WindowView::Renewed {
                    resets_at: 12 * H,
                    read_at: 9 * H
                }
            );
            // A semanal, com reset diferente, segue valendo.
            assert!(matches!(v.seven_day, WindowView::Ok { .. }));
        }
        let json = serde_json::to_value(plan_view(&r, 13 * H)).unwrap();
        assert_eq!(json["five_hour"]["state"], "renewed");
        assert!(json["five_hour"].get("used_percentage").is_none());
    }

    #[test]
    fn mais_recente_por_janela_nao_o_maior_percentual() {
        let now = 10 * H;
        let v = plan_view(
            &[
                reading(8 * H, Some((90.0, 12 * H)), Some((70.0, 100 * H))),
                // Mais nova, mas sem a semanal.
                reading(9 * H, Some((40.0, 14 * H)), None),
            ],
            now,
        );
        assert!(
            matches!(v.five_hour, WindowView::Ok { used_percentage, read_at, .. }
            if used_percentage == 40.0 && read_at == 9 * H)
        );
        assert!(
            matches!(v.seven_day, WindowView::Ok { used_percentage, read_at, .. }
            if used_percentage == 70.0 && read_at == 8 * H)
        );
        assert_eq!(v.read_at, Some(9 * H));
    }

    #[test]
    fn sem_leitura() {
        let v = plan_view(&[], 0);
        assert_eq!(v.five_hour, WindowView::None);
        assert_eq!(v.seven_day, WindowView::None);
        assert_eq!(v.read_at, None);
        assert_eq!(merge_readings(&[]), None);
    }

    #[test]
    fn merge_por_janela() {
        let m = merge_readings(&[
            reading(8 * H, Some((90.0, 12 * H)), Some((70.0, 100 * H))),
            reading(9 * H, Some((40.0, 14 * H)), None),
        ])
        .unwrap();
        assert_eq!(m.read_at, 9 * H);
        assert_eq!(m.five_hour.unwrap().used_percentage, 40.0);
        assert_eq!(m.seven_day.unwrap().used_percentage, 70.0);
    }

    #[test]
    fn agrupa_ambientes_pela_conta() {
        let now = 10 * H;
        let report = |key: Option<&str>, r: Option<PlanReading>| EnvPlanReport {
            account: key.map(account),
            statusline: StatuslineHealth::default(),
            latest: r,
        };
        let (accounts, unknown) = group_accounts(
            [
                (
                    "e1".into(),
                    "env-1".into(),
                    report(
                        Some("conta-a"),
                        Some(reading(8 * H, Some((90.0, 12 * H)), None)),
                    ),
                ),
                (
                    "e2".into(),
                    "env-2".into(),
                    report(
                        Some("conta-a"),
                        Some(reading(9 * H, Some((20.0, 14 * H)), None)),
                    ),
                ),
                (
                    "e3".into(),
                    "env-3".into(),
                    report(
                        Some("conta-b"),
                        Some(reading(9 * H, Some((55.0, 12 * H)), None)),
                    ),
                ),
                (
                    "e4".into(),
                    "env-4".into(),
                    report(None, Some(reading(9 * H, Some((99.0, 12 * H)), None))),
                ),
            ],
            now,
        );
        assert_eq!(accounts.len(), 2);
        let a = &accounts[0];
        assert_eq!(a.account.key, "conta-a");
        assert_eq!(
            a.envs.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(),
            ["env-1", "env-2"]
        );
        // A leitura mais recente da conta, de qualquer ambiente.
        assert!(
            matches!(a.plan.five_hour, WindowView::Ok { used_percentage, .. } if used_percentage == 20.0)
        );
        // Conta B não recebe leitura da A nem da desconhecida.
        assert!(
            matches!(accounts[1].plan.five_hour, WindowView::Ok { used_percentage, .. } if used_percentage == 55.0)
        );
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].name, "env-4");
    }

    #[test]
    fn statusline_desatualizado() {
        let h = StatuslineHealth {
            installed: true,
            version: Some(0),
            current: STATUSLINE_VERSION,
            ..Default::default()
        };
        assert!(h.outdated());
        assert!(!StatuslineHealth {
            version: Some(STATUSLINE_VERSION),
            ..h.clone()
        }
        .outdated());
        assert!(!StatuslineHealth::default().outdated());
    }
}
