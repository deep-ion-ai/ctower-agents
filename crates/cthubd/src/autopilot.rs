//! Orçamento do piloto automático por conta (issue #180, épico #178):
//! `hub.autopilot.policy.get|set|preview` no `/ws` (só desktop: a web não
//! recebe a chave da conta) e [`account_decision`], que o agendador (#183)
//! chama antes de iniciar cada issue.
//!
//! A regra mora em `ct_core::plan::budget` (pura); aqui o hub junta a
//! política guardada (`autopilot_policy`) com a visão do plano da conta
//! (`ct_core::plan::group_accounts`, a leitura mais recente de qualquer
//! ambiente, com a regra do reset). Conta sem política = piloto desligado.

use crate::HubState;
use ct_core::plan::budget::{evaluate, BudgetDecision, BudgetPolicy};
use ct_core::plan::{group_accounts, PlanView, WindowView};
use ct_http::rpc::{invalid, str_param, RpcResult};
use ct_ledger::Hub;
use serde::Serialize;
use serde_json::{json, Value};

/// Resposta de `hub.autopilot.policy.get|set`.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AutopilotPolicyView {
    /// Chave da conta (`PlanAccount.key`).
    pub account: String,
    /// Política guardada; `None` = piloto automático desligado na conta.
    pub policy: Option<BudgetPolicy>,
    /// Política padrão (ponto de partida do editor).
    pub default: BudgetPolicy,
    /// Avaliação da política guardada agora; `None` sem política.
    pub decision: Option<BudgetDecision>,
}

/// Visão do plano da conta em `now`; conta sem ambiente que informou = sem
/// leitura.
pub fn account_plan(hub: &Hub, account_key: &str, now: i64) -> ct_ledger::Result<PlanView> {
    let (accounts, _) = group_accounts(hub.list_env_plans()?, now);
    Ok(accounts
        .into_iter()
        .find(|a| a.account.key == account_key)
        .map(|a| a.plan)
        .unwrap_or(PlanView {
            five_hour: WindowView::None,
            seven_day: WindowView::None,
            read_at: None,
        }))
}

/// Pode iniciar uma sessão autônoma nova na conta agora? `None` = conta sem
/// política (piloto automático desligado: o agendador não inicia nada).
/// Para o agendador (#183): a decisão traz os motivos e `next_open_at`.
pub fn account_decision(
    hub: &Hub,
    account_key: &str,
    now: i64,
) -> ct_ledger::Result<Option<BudgetDecision>> {
    let Some(policy) = hub.autopilot_policy(account_key)? else {
        return Ok(None);
    };
    let plan = account_plan(hub, account_key, now)?;
    Ok(Some(evaluate(&policy, &plan, now)))
}

fn view(hub: &Hub, account: &str, now: i64) -> RpcResult {
    let e = |e: ct_ledger::LedgerError| (-32000, e.to_string());
    let policy = hub.autopilot_policy(account).map_err(e)?;
    let decision = match &policy {
        Some(p) => Some(evaluate(
            p,
            &account_plan(hub, account, now).map_err(e)?,
            now,
        )),
        None => None,
    };
    Ok(json!(AutopilotPolicyView {
        account: account.to_string(),
        policy,
        default: BudgetPolicy::default(),
        decision,
    }))
}

fn policy_param(p: &Value) -> Result<Option<BudgetPolicy>, (i64, String)> {
    match p.get("policy") {
        None => Err(invalid("faltou policy (null desliga o piloto automático)")),
        Some(Value::Null) => Ok(None),
        Some(v) => {
            let policy: BudgetPolicy = serde_json::from_value(v.clone())
                .map_err(|e| invalid(format!("policy inválida: {e}")))?;
            policy.validate().map_err(invalid)?;
            Ok(Some(policy))
        }
    }
}

/// `hub.autopilot.policy.get { account }`, `.set { account, policy }`
/// (`policy: null` desliga) e `.preview { account, policy }` (avalia sem
/// gravar: a prévia do editor). Síncrono: roda fora do runtime.
pub(crate) fn call(st: &HubState, method: &str, p: &Value) -> RpcResult {
    let account = str_param(p, "account")?.trim().to_string();
    if account.is_empty() {
        return Err(invalid("account vazio"));
    }
    let now = ct_core::now_ms();
    let hub = st.hub.lock();
    match method {
        "hub.autopilot.policy.get" => view(&hub, &account, now),
        "hub.autopilot.policy.set" => {
            let policy = policy_param(p)?;
            hub.set_autopilot_policy(&account, policy.as_ref())
                .map_err(|e| (-32000, e.to_string()))?;
            view(&hub, &account, now)
        }
        "hub.autopilot.policy.preview" => {
            let policy = policy_param(p)?.ok_or_else(|| invalid("faltou policy"))?;
            let plan = account_plan(&hub, &account, now).map_err(|e| (-32000, e.to_string()))?;
            Ok(json!(evaluate(&policy, &plan, now)))
        }
        _ => Err((-32601, format!("método desconhecido: {method}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_core::plan::{EnvPlanReport, PlanAccount, PlanReading, PlanWindow};

    fn hub_with_reading(now: i64) -> Hub {
        let h = Hub::open_in_memory().unwrap();
        h.ingest("e1", "env-1", &[]).unwrap();
        h.set_env_plan(
            "e1",
            &EnvPlanReport {
                account: Some(PlanAccount {
                    key: "conta-a".into(),
                    email: Some("fulano@example.com".into()),
                    org_name: None,
                    subscription_type: None,
                }),
                statusline: Default::default(),
                latest: Some(PlanReading {
                    read_at: now - 1000,
                    five_hour: Some(PlanWindow {
                        used_percentage: 40.0,
                        resets_at: now + 3_600_000,
                    }),
                    seven_day: Some(PlanWindow {
                        used_percentage: 30.0,
                        resets_at: now + 2 * 86_400_000,
                    }),
                }),
            },
        )
        .unwrap();
        h
    }

    #[test]
    fn sem_politica_o_piloto_fica_desligado() {
        let now = ct_core::now_ms();
        let h = hub_with_reading(now);
        assert_eq!(account_decision(&h, "conta-a", now).unwrap(), None);
        h.set_autopilot_policy("conta-a", Some(&BudgetPolicy::default()))
            .unwrap();
        let d = account_decision(&h, "conta-a", now).unwrap().unwrap();
        assert_eq!(d.five_hour_used, Some(40.0));
        assert_eq!(d.weekly_used, Some(30.0));
        // Conta sem leitura: avalia, mas bloqueia por falta de leitura.
        h.set_autopilot_policy("conta-b", Some(&BudgetPolicy::default()))
            .unwrap();
        let d = account_decision(&h, "conta-b", now).unwrap().unwrap();
        assert!(!d.can_start);
    }

    #[test]
    fn metodos_get_set_preview() {
        let now = ct_core::now_ms();
        let st = HubState::new(
            ct_ledger::SharedHub::new(hub_with_reading(now)),
            "ui",
            "hub",
            "hub-id",
        );
        let acc = json!({ "account": "conta-a" });
        let v = call(&st, "hub.autopilot.policy.get", &acc).unwrap();
        assert_eq!(v["policy"], Value::Null);
        assert_eq!(v["decision"], Value::Null);
        assert_eq!(v["default"]["weekly_cap"], 80.0);
        assert_eq!(v["default"]["utc_offset_min"], -180);

        let default = serde_json::to_value(BudgetPolicy::default()).unwrap();
        let v = call(
            &st,
            "hub.autopilot.policy.preview",
            &json!({ "account": "conta-a", "policy": default }),
        )
        .unwrap();
        assert_eq!(v["weekly_used"], 30.0);
        // A prévia não grava.
        let v = call(&st, "hub.autopilot.policy.get", &acc).unwrap();
        assert_eq!(v["policy"], Value::Null);

        let v = call(
            &st,
            "hub.autopilot.policy.set",
            &json!({ "account": "conta-a", "policy": default }),
        )
        .unwrap();
        assert_eq!(v["policy"]["reserve_per_workday"], 12.0);
        assert!(v["decision"]["can_start"].is_boolean());

        let mut ruim = default.clone();
        ruim["weekly_cap"] = json!(101);
        let e = call(
            &st,
            "hub.autopilot.policy.set",
            &json!({ "account": "conta-a", "policy": ruim }),
        )
        .unwrap_err();
        assert!(e.1.contains("teto semanal"), "{e:?}");
        assert!(call(&st, "hub.autopilot.policy.set", &acc).is_err());

        let v = call(
            &st,
            "hub.autopilot.policy.set",
            &json!({ "account": "conta-a", "policy": null }),
        )
        .unwrap();
        assert_eq!(v["policy"], Value::Null);
        assert!(call(&st, "hub.autopilot.policy.get", &json!({})).is_err());
    }
}
