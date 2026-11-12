//! Resumo da manhã do piloto automático (#185, épico #178), gerado pelo
//! hub: o laço do agendador ([`super::runner::spawn`]) chama [`run`] a cada
//! ciclo, sem depender da UI aberta.
//!
//! - **Quando:** em todo dia útil, no horário da política
//!   (`BudgetPolicy::digest_at_min`, padrão 09:00 no fuso dela; nulo =
//!   sem resumo), uma vez por dia ([`BudgetPolicy::digest_due`], com o
//!   último dia guardado no `hub.db`). Hub fora do ar no horário: sai
//!   quando ele voltar, no mesmo dia. Nunca em fim de semana.
//! - **Qual política:** o resumo é um só para o hub. Vale a política da
//!   primeira conta (pela chave) com política; sem política nenhuma, o
//!   piloto está desligado e não há resumo. Sem projeto no modo autônomo,
//!   também não.
//! - **Conteúdo:** desde o resumo anterior (o primeiro cobre as últimas
//!   24 h), por projeto autônomo, as issues mescladas na branch de
//!   integração, as bloqueadas, as decisões autônomas sem revisão, as
//!   issues à frente da base e o PR `branch → base` com o estado do CI do
//!   espelho; e, por conta com política, o uso da semanal no período.
//!   Nada da conversa do agente (veja `ct_ledger::MorningDigest`).
//! - **Avisos:** o resumo novo vira o push `hub.autopilot.changed` às UIs
//!   (o card na Visão geral) e a Web Push `morning_digest`.

use ct_core::plan::budget::BudgetPolicy;
use ct_core::plan::WindowView;
use ct_ledger::{DigestUsage, Hub, MorningDigest};
use std::collections::BTreeMap;

const DAY_MS: i64 = 86_400_000;

/// A política que governa o resumo: a da primeira conta (pela chave).
pub fn digest_policy(hub: &Hub) -> ct_ledger::Result<Option<(String, BudgetPolicy)>> {
    Ok(hub.list_autopilot_policies()?.into_iter().next())
}

/// Uso da semanal da conta agora: `(uso, reset)`; renovada = 0%.
fn weekly_now(hub: &Hub, key: &str, now: i64) -> ct_ledger::Result<(Option<f64>, Option<i64>)> {
    Ok(match super::account_plan(hub, key, now)?.seven_day {
        WindowView::Ok {
            used_percentage,
            resets_at,
            ..
        } => (Some(used_percentage), Some(resets_at)),
        WindowView::Renewed { .. } => (Some(0.0), None),
        WindowView::None => (None, None),
    })
}

/// Monta o resumo do dia `day` em `now` (sem gravar).
pub fn compose(hub: &Hub, day: i64, now: i64) -> ct_ledger::Result<MorningDigest> {
    let prev = hub.list_digests(1)?.into_iter().next();
    let since = prev.as_ref().map_or(now - DAY_MS, |p| p.at);
    let mut usage = Vec::new();
    for (key, _) in hub.list_autopilot_policies()? {
        let (used, resets_at) = weekly_now(hub, &key, now)?;
        let before = prev
            .as_ref()
            .and_then(|p| p.usage.iter().find(|u| u.account == key));
        let weekly_before = before.and_then(|b| b.weekly_used);
        let delta = match (used, before) {
            (None, _) => None,
            (Some(u), Some(b)) if b.resets_at.is_some() && b.resets_at == resets_at => {
                Some((u - b.weekly_used.unwrap_or(0.0)).max(0.0))
            }
            // Sem resumo anterior, ou a semanal renovou no meio: o uso
            // atual (desde o reset ou o início da leitura).
            (Some(u), _) => Some(u),
        };
        usage.push(DigestUsage {
            account: key,
            weekly_used: used,
            resets_at,
            weekly_before,
            delta,
        });
    }
    Ok(MorningDigest {
        id: 0,
        local_day: day,
        at: now,
        since,
        projects: hub.digest_projects(since, now)?,
        usage,
    })
}

/// Gera e grava o resumo se ele estiver devido em `now`; `Some` = saiu um
/// resumo novo (o chamador avisa as UIs e a Web Push).
pub fn run(hub: &Hub, now: i64) -> ct_ledger::Result<Option<MorningDigest>> {
    let Some((_, policy)) = digest_policy(hub)? else {
        return Ok(None);
    };
    let Some(day) = policy.digest_due(now, hub.last_digest_day()?) else {
        return Ok(None);
    };
    if !hub.list_projects()?.iter().any(|p| p.autonomy.enabled) {
        return Ok(None);
    }
    let d = compose(hub, day, now)?;
    hub.save_digest(&d)
}

/// Na web, as chaves de conta viram `conta-N`, na mesma numeração do
/// `hub.autopilot.status` (as contas dos ambientes, pela chave).
pub fn anonymize(hub: &Hub, digests: &mut [MorningDigest]) -> ct_ledger::Result<()> {
    let keys: std::collections::BTreeSet<String> = hub
        .list_env_plans()?
        .into_iter()
        .filter_map(|(_, _, r)| r.account.map(|a| a.key))
        .collect();
    let alias: BTreeMap<String, String> = keys
        .into_iter()
        .enumerate()
        .map(|(i, k)| (k, format!("conta-{}", i + 1)))
        .collect();
    for d in digests {
        for u in &mut d.usage {
            u.account = alias
                .get(&u.account)
                .cloned()
                .unwrap_or_else(|| "conta".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ct_core::plan::{EnvPlanReport, PlanAccount, PlanReading, PlanWindow};
    use ct_core::AutonomyConfig;
    use ct_ledger::{HubProjectPatch, NewHubProject};

    const H: i64 = 3_600_000;
    const MIN: i64 = 60_000;
    /// Segunda, 2026-11-09 00:00 em GMT-3 (03:00 UTC): relógio simulado.
    const SEG: i64 = 1_794_193_200_000;

    fn at(day: i64, h: i64, m: i64) -> i64 {
        SEG + day * DAY_MS + h * H + m * MIN
    }

    fn reading(hub: &Hub, weekly: f64, resets_at: i64, now: i64) {
        hub.set_env_plan(
            "env-auto",
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
                        used_percentage: 10.0,
                        resets_at: now + H,
                    }),
                    seven_day: Some(PlanWindow {
                        used_percentage: weekly,
                        resets_at,
                    }),
                }),
            },
        )
        .unwrap();
    }

    /// `acme/acme-web` no modo autônomo e a conta `conta-a` com a política
    /// padrão (resumo às 09:00, GMT-3).
    fn hub() -> Hub {
        let hub = Hub::open_in_memory().unwrap();
        hub.ingest("env-auto", "env-auto", &[]).unwrap();
        hub.add_project(&NewHubProject {
            owner: "acme".into(),
            repo: "acme-web".into(),
            name: "acme-web".into(),
            base_branch: "develop".into(),
            protected_branches: vec!["develop".into()],
        })
        .unwrap();
        hub.set_env_autonomous("env-auto", true).unwrap();
        hub.add_assignment("acme", "acme-web", "env-auto", None)
            .unwrap();
        hub.update_project(
            "acme",
            "acme-web",
            &HubProjectPatch {
                autonomy: Some(AutonomyConfig {
                    enabled: true,
                    env_id: Some("env-auto".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .unwrap();
        hub.set_autopilot_policy("conta-a", Some(&BudgetPolicy::default()))
            .unwrap();
        hub
    }

    #[test]
    fn sai_no_horario_uma_vez_por_dia_util() {
        let hub = hub();
        reading(&hub, 20.0, at(4, 12, 0), at(0, 8, 0));
        // Segunda 08:59: ainda não.
        assert!(run(&hub, at(0, 8, 59)).unwrap().is_none());
        let d = run(&hub, at(0, 9, 0)).unwrap().unwrap();
        assert_eq!(d.at, at(0, 9, 0));
        assert_eq!(d.since, at(0, 9, 0) - DAY_MS);
        assert_eq!(d.projects.len(), 1);
        assert_eq!(d.projects[0].branch, "wip/autonoma");
        // O laço roda a cada minuto: não repete no dia.
        for m in [1, 30, 600] {
            assert!(run(&hub, at(0, 9, 0) + m * MIN).unwrap().is_none());
        }
        // Terça: o período começa no resumo de segunda; o uso subiu 15 pontos.
        reading(&hub, 35.0, at(4, 12, 0), at(1, 8, 0));
        let t = run(&hub, at(1, 9, 5)).unwrap().unwrap();
        assert_eq!(t.since, at(0, 9, 0));
        let u = &t.usage[0];
        assert_eq!(u.account, "conta-a");
        assert_eq!(
            (u.weekly_used, u.weekly_before, u.delta),
            (Some(35.0), Some(20.0), Some(15.0))
        );
        assert_eq!(hub.list_digests(10).unwrap().len(), 2);
    }

    #[test]
    fn nunca_no_fim_de_semana() {
        let hub = hub();
        reading(&hub, 20.0, at(9, 12, 0), at(4, 8, 0));
        run(&hub, at(4, 9, 0)).unwrap().unwrap();
        for d in [5, 6] {
            for h in [0, 9, 12, 23] {
                assert!(run(&hub, at(d, h, 0)).unwrap().is_none(), "dia {d} {h}h");
            }
        }
        // Segunda seguinte, às 09:00 no fuso da política (12:00 UTC).
        let seg = run(&hub, at(7, 9, 0)).unwrap().unwrap();
        assert_eq!(seg.since, at(4, 9, 0));
    }

    #[test]
    fn sem_politica_sem_horario_ou_sem_projeto_autonomo_nao_sai() {
        let hub = hub();
        let sem_horario = BudgetPolicy {
            digest_at_min: None,
            ..BudgetPolicy::default()
        };
        hub.set_autopilot_policy("conta-a", Some(&sem_horario))
            .unwrap();
        assert!(run(&hub, at(0, 10, 0)).unwrap().is_none());
        hub.set_autopilot_policy("conta-a", None).unwrap();
        assert!(run(&hub, at(0, 10, 0)).unwrap().is_none());
        hub.set_autopilot_policy("conta-a", Some(&BudgetPolicy::default()))
            .unwrap();
        hub.update_project(
            "acme",
            "acme-web",
            &HubProjectPatch {
                autonomy: Some(AutonomyConfig::default()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(run(&hub, at(0, 10, 0)).unwrap().is_none());
    }

    #[test]
    fn semanal_renovada_conta_desde_o_reset_e_web_anonimiza() {
        let hub = hub();
        reading(&hub, 70.0, at(1, 6, 0), at(0, 8, 0));
        run(&hub, at(0, 9, 0)).unwrap().unwrap();
        // Renovou terça 06:00; na leitura nova, 5%.
        reading(&hub, 5.0, at(8, 6, 0), at(1, 8, 0));
        let mut d = vec![run(&hub, at(1, 9, 0)).unwrap().unwrap()];
        assert_eq!(d[0].usage[0].delta, Some(5.0));
        anonymize(&hub, &mut d).unwrap();
        assert_eq!(d[0].usage[0].account, "conta-1");
        let json = serde_json::to_string(&d).unwrap();
        assert!(!json.contains("conta-a") && !json.contains("example.com"));
    }
}
