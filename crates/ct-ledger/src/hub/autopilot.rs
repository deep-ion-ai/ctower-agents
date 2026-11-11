//! Política de orçamento do piloto automático por conta (issue #180,
//! épico #178). A avaliação é pura, em `ct_core::plan::budget`; aqui só se
//! guarda a política de cada conta (`account_key` de `ct_core::plan`).
//! Conta sem política = piloto automático desligado para ela.

use super::Hub;
use crate::{LedgerError, Result};
use ct_core::now_ms;
use ct_core::plan::budget::BudgetPolicy;
use rusqlite::{params, OptionalExtension};

impl Hub {
    /// Política da conta; `None` = não configurada (piloto desligado). Um
    /// JSON ilegível também conta como não configurada.
    pub fn autopilot_policy(&self, account_key: &str) -> Result<Option<BudgetPolicy>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT policy FROM autopilot_policy WHERE account_key = ?1",
                [account_key],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw.and_then(|r| serde_json::from_str(&r).ok()))
    }

    /// Grava (ou, com `None`, apaga) a política da conta. A política é
    /// validada antes ([`BudgetPolicy::validate`]); inválida é recusada
    /// com [`LedgerError::Conflict`] e o motivo.
    pub fn set_autopilot_policy(
        &self,
        account_key: &str,
        policy: Option<&BudgetPolicy>,
    ) -> Result<()> {
        if account_key.trim().is_empty() {
            return Err(LedgerError::Conflict("conta vazia".into()));
        }
        match policy {
            None => {
                self.conn.execute(
                    "DELETE FROM autopilot_policy WHERE account_key = ?1",
                    [account_key],
                )?;
            }
            Some(p) => {
                p.validate().map_err(LedgerError::Conflict)?;
                let json = serde_json::to_string(p).expect("BudgetPolicy serializa");
                self.conn.execute(
                    "INSERT INTO autopilot_policy (account_key, policy, updated_at)
                     VALUES (?1, ?2, ?3)
                     ON CONFLICT (account_key) DO UPDATE
                     SET policy = excluded.policy, updated_at = excluded.updated_at",
                    params![account_key, json, now_ms()],
                )?;
            }
        }
        Ok(())
    }

    /// Todas as políticas configuradas, por conta (o agendador, #183,
    /// percorre só estas: as outras contas ficam com o piloto desligado).
    pub fn list_autopilot_policies(&self) -> Result<Vec<(String, BudgetPolicy)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT account_key, policy FROM autopilot_policy ORDER BY account_key")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (key, raw) = row?;
            if let Ok(p) = serde_json::from_str(&raw) {
                out.push((key, p));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use crate::Hub;
    use ct_core::plan::budget::BudgetPolicy;

    #[test]
    fn sem_politica_ate_gravar_e_apagar() {
        let h = Hub::open_in_memory().unwrap();
        assert_eq!(h.autopilot_policy("conta-a").unwrap(), None);
        assert!(h.list_autopilot_policies().unwrap().is_empty());
        let mut p = BudgetPolicy::default();
        h.set_autopilot_policy("conta-a", Some(&p)).unwrap();
        assert_eq!(h.autopilot_policy("conta-a").unwrap(), Some(p.clone()));
        p.weekly_cap = 70.0;
        h.set_autopilot_policy("conta-a", Some(&p)).unwrap();
        assert_eq!(
            h.autopilot_policy("conta-a").unwrap().unwrap().weekly_cap,
            70.0
        );
        assert_eq!(h.autopilot_policy("conta-b").unwrap(), None);
        assert_eq!(h.list_autopilot_policies().unwrap().len(), 1);
        h.set_autopilot_policy("conta-a", None).unwrap();
        assert_eq!(h.autopilot_policy("conta-a").unwrap(), None);
    }

    #[test]
    fn politica_invalida_nao_grava() {
        let h = Hub::open_in_memory().unwrap();
        let p = BudgetPolicy {
            weekly_cap: 150.0,
            ..BudgetPolicy::default()
        };
        assert!(h.set_autopilot_policy("conta-a", Some(&p)).is_err());
        assert!(h
            .set_autopilot_policy(" ", Some(&BudgetPolicy::default()))
            .is_err());
        assert_eq!(h.autopilot_policy("conta-a").unwrap(), None);
    }
}
