//! Chave pública age de cada ambiente para a entrega de secrets (issue
//! #184, épico #171). O ambiente manda a chave em todo `GET /sync/config`
//! (só o token preso ao `env_id` chega aqui); o hub guarda a última e a da
//! última entrega. Chave atual diferente da entregue = reentregar, mesmo sem
//! mudança de `config_rev`.
//!
//! Para a UI (#165), o hub guarda também o `config_rev` que o ambiente
//! confirmou ter aplicado: o `since` do pull. O ambiente só avança o
//! `since` depois de gravar o cache de secrets, então um `since` na época
//! atual, vindo de um ambiente cuja chave já recebeu entrega, confirma que
//! os secrets daquela revisão chegaram.

use super::Hub;
use crate::Result;
use ct_core::now_ms;
use rusqlite::{params, OptionalExtension};

/// Chave pública registrada para um ambiente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvPubkey {
    /// `age1...` mandada no último pull.
    pub pubkey: String,
    /// Chave para a qual o hub fez a última entrega; `None` = nunca entregou.
    pub delivered: Option<String>,
}

impl EnvPubkey {
    /// A chave mudou desde a última entrega (ou nunca houve entrega).
    pub fn needs_delivery(&self) -> bool {
        self.delivered.as_deref() != Some(self.pubkey.as_str())
    }
}

/// O que o hub sabe de um ambiente para dizer se os secrets chegaram lá
/// (`hub.secrets.list`, #165). Quem chama cruza com o store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvSecretsInput {
    pub env_id: String,
    pub env_name: String,
    pub key: Option<EnvPubkey>,
    /// `config_rev` confirmado pelo ambiente na época atual.
    pub applied_rev: Option<i64>,
    /// Atribuições do ambiente, inclusive as removidas e as de projetos
    /// excluídos: atribuir ou tirar um projeto muda os secrets entregues.
    pub assignments: Vec<EnvAssignmentRev>,
}

/// Uma atribuição vista pela entrega de secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvAssignmentRev {
    /// `owner/repo` em minúsculas.
    pub project_key: String,
    /// `config_rev` da última mudança da atribuição ou do projeto.
    pub rev: i64,
    /// Atribuição ativa e projeto não excluído (os secrets dele vão para o
    /// ambiente).
    pub active: bool,
}

impl Hub {
    /// Registra a chave pública do ambiente (já validada por quem chama).
    pub fn set_env_pubkey(&self, env_id: &str, pubkey: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO env_key (env_id, pubkey, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (env_id) DO UPDATE SET pubkey = excluded.pubkey,
                updated_at = excluded.updated_at
             WHERE env_key.pubkey <> excluded.pubkey",
            params![env_id, pubkey, now_ms()],
        )?;
        Ok(())
    }

    pub fn env_pubkey(&self, env_id: &str) -> Result<Option<EnvPubkey>> {
        Ok(self
            .conn
            .query_row(
                "SELECT pubkey, delivered_pubkey FROM env_key WHERE env_id = ?1",
                [env_id],
                |r| {
                    Ok(EnvPubkey {
                        pubkey: r.get(0)?,
                        delivered: r.get(1)?,
                    })
                },
            )
            .optional()?)
    }

    /// Anota o `config_rev` (`since`) que o ambiente diz ter aplicado. Só
    /// vale na época atual e para uma chave que já recebeu entrega (com a
    /// chave trocada, o `since` não diz nada sobre os secrets).
    pub fn record_env_applied(
        &self,
        env_id: &str,
        since: Option<i64>,
        epoch: Option<i64>,
    ) -> Result<()> {
        let (Some(rev), Some(epoch)) = (since, epoch) else {
            return Ok(());
        };
        if epoch != self.config_epoch()? {
            return Ok(());
        }
        self.conn.execute(
            "UPDATE env_key SET applied_rev = ?2, applied_epoch = ?3
             WHERE env_id = ?1 AND delivered_pubkey = pubkey",
            params![env_id, rev, epoch],
        )?;
        Ok(())
    }

    /// Entrada do estado de entrega de secrets de cada ambiente conhecido
    /// (já falou com o hub ou tem token preso a um `env_id`), por nome.
    pub fn secrets_delivery_inputs(&self) -> Result<Vec<EnvSecretsInput>> {
        let epoch = self.config_epoch()?;
        let envs: Vec<(String, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT env_id, name FROM env WHERE last_seen_at IS NOT NULL
                 UNION
                 SELECT env_id, name FROM env_token
                 WHERE env_id IS NOT NULL
                   AND env_id NOT IN (SELECT env_id FROM env WHERE last_seen_at IS NOT NULL)
                 ORDER BY 2, 1",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut out = Vec::with_capacity(envs.len());
        for (env_id, env_name) in envs {
            let applied_rev: Option<i64> = self
                .conn
                .query_row(
                    "SELECT applied_rev FROM env_key WHERE env_id = ?1 AND applied_epoch = ?2",
                    params![env_id, epoch],
                    |r| r.get(0),
                )
                .optional()?
                .flatten();
            let assignments = {
                let mut stmt = self.conn.prepare(
                    "SELECT p.key, max(a.rev, p.rev),
                            a.removed_at IS NULL AND p.deleted_at IS NULL
                     FROM assignment a JOIN project p ON p.id = a.project_id
                     WHERE a.env_id = ?1 ORDER BY p.key",
                )?;
                let rows = stmt.query_map([&env_id], |r| {
                    Ok(EnvAssignmentRev {
                        project_key: r.get(0)?,
                        rev: r.get(1)?,
                        active: r.get(2)?,
                    })
                })?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            out.push(EnvSecretsInput {
                key: self.env_pubkey(&env_id)?,
                env_id,
                env_name,
                applied_rev,
                assignments,
            });
        }
        Ok(out)
    }

    /// Anota que o hub entregou os secrets cifrados para `pubkey`.
    pub fn mark_secrets_delivered(&self, env_id: &str, pubkey: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE env_key SET delivered_pubkey = ?2 WHERE env_id = ?1",
            params![env_id, pubkey],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::Hub;

    #[test]
    fn chave_registrada_entregue_e_apagada_na_revogacao() {
        let h = Hub::open_in_memory().unwrap();
        assert!(h.env_pubkey("e").unwrap().is_none());
        h.set_env_pubkey("e", "age1a").unwrap();
        let k = h.env_pubkey("e").unwrap().unwrap();
        assert_eq!((k.pubkey.as_str(), k.delivered.as_deref()), ("age1a", None));
        assert!(k.needs_delivery());
        h.mark_secrets_delivered("e", "age1a").unwrap();
        assert!(!h.env_pubkey("e").unwrap().unwrap().needs_delivery());
        h.set_env_pubkey("e", "age1a").unwrap();
        assert!(!h.env_pubkey("e").unwrap().unwrap().needs_delivery());
        h.set_env_pubkey("e", "age1b").unwrap();
        assert!(h.env_pubkey("e").unwrap().unwrap().needs_delivery());

        // Revogar o token preso a `e` apaga a chave; outro ambiente fica.
        h.add_bound_env_token("wsl", "e").unwrap();
        h.set_env_pubkey("f", "age1f").unwrap();
        assert!(h.revoke_env_token("wsl").unwrap());
        assert!(h.env_pubkey("e").unwrap().is_none());
        assert!(h.env_pubkey("f").unwrap().is_some());
    }

    #[test]
    fn applied_rev_so_com_chave_entregue_e_na_epoca_atual() {
        let h = Hub::open_in_memory().unwrap();
        let epoch = h.config_epoch().unwrap();
        h.add_bound_env_token("wsl", "e").unwrap();
        let inp = |h: &Hub| h.secrets_delivery_inputs().unwrap();
        assert_eq!(inp(&h)[0].env_name, "wsl");
        assert!(inp(&h)[0].key.is_none());

        // Sem entrega para a chave atual, o `since` não conta.
        h.set_env_pubkey("e", "age1a").unwrap();
        h.record_env_applied("e", Some(3), Some(epoch)).unwrap();
        assert_eq!(inp(&h)[0].applied_rev, None);
        h.mark_secrets_delivered("e", "age1a").unwrap();
        h.record_env_applied("e", Some(3), Some(epoch)).unwrap();
        assert_eq!(inp(&h)[0].applied_rev, Some(3));
        // Outra época (restore) ou sem `since`: nada muda.
        h.record_env_applied("e", Some(9), Some(epoch + 1)).unwrap();
        h.record_env_applied("e", None, Some(epoch)).unwrap();
        assert_eq!(inp(&h)[0].applied_rev, Some(3));
        h.renew_config_epoch().unwrap();
        assert_eq!(inp(&h)[0].applied_rev, None);
    }

    #[test]
    fn config_rev_passa_do_minimo_do_restore() {
        let h = Hub::open_in_memory().unwrap();
        // hub.db novo: passa do maior rev do store restaurado.
        assert_eq!(h.bump_config_rev_past(7).unwrap(), 8);
        assert_eq!(h.config_rev().unwrap(), 8);
        // Já acima do mínimo: só incrementa.
        assert_eq!(h.bump_config_rev_past(3).unwrap(), 9);
        assert_eq!(h.bump_config_rev_for_secrets().unwrap(), 10);
        assert_eq!(h.bump_config_rev_past(-1).unwrap(), 11);
    }
}
