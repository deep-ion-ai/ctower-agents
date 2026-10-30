//! Sessões web do hub (issue #200, spec da PWA §3.1 e §9.1): o cookie
//! `ct_web=<id>.<segredo>` criado pelo `POST /web/login` com o token de UI.
//! O hub guarda só o SHA-256 do segredo, como nos tokens de sync e de
//! agente. Uma sessão vale até `expires_at` (validade absoluta) e enquanto o
//! último uso tiver menos que a inatividade máxima; revogada, não volta.

use super::{token_hash, Hub};
use crate::Result;
use ct_core::now_ms;
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

/// O uso só é gravado de novo depois desse intervalo (spec §3.1).
pub const WEB_SESSION_TOUCH_MS: i64 = 60 * 1000;
/// Sessões vencidas ou revogadas há mais que isso saem da tabela.
const WEB_SESSION_PRUNE_MS: i64 = 30 * 24 * 3600 * 1000;
/// Teto do agente do navegador e da identidade do proxy guardados.
const TEXT_MAX: usize = 200;

/// Sessão web em `hub.web.sessions.list` (sem o segredo nem o hash).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubWebSession {
    pub id: String,
    pub created_at: i64,
    /// Último uso, gravado no máximo uma vez por minuto.
    pub last_used_at: i64,
    /// Validade absoluta.
    pub expires_at: i64,
    /// Identidade do proxy no login (e-mail do Access ou login do
    /// Tailscale); `None` = sem proxy.
    pub identity: Option<String>,
    /// `User-Agent` do navegador no login.
    pub user_agent: Option<String>,
    /// É a sessão da própria conexão (só no `/web/ws`).
    #[serde(default)]
    pub current: bool,
}

/// Sessão recém-criada: o valor do cookie só existe aqui.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedWebSession {
    pub id: String,
    /// `<id>.<segredo>`, o valor do cookie `ct_web`.
    pub cookie: String,
    pub expires_at: i64,
}

const COLS: &str = "id, created_at, last_used_at, expires_at, identity, user_agent";

fn row_to_session(r: &Row) -> rusqlite::Result<HubWebSession> {
    Ok(HubWebSession {
        id: r.get(0)?,
        created_at: r.get(1)?,
        last_used_at: r.get(2)?,
        expires_at: r.get(3)?,
        identity: r.get(4)?,
        user_agent: r.get(5)?,
        current: false,
    })
}

fn clip(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(TEXT_MAX).collect())
}

fn eq_ct(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl Hub {
    /// Cria uma sessão web válida por `session_ms` a partir de agora.
    pub fn create_web_session(
        &self,
        identity: Option<&str>,
        user_agent: Option<&str>,
        session_ms: i64,
    ) -> Result<IssuedWebSession> {
        self.create_web_session_at(identity, user_agent, session_ms, now_ms())
    }

    pub fn create_web_session_at(
        &self,
        identity: Option<&str>,
        user_agent: Option<&str>,
        session_ms: i64,
        now: i64,
    ) -> Result<IssuedWebSession> {
        let id = uuid::Uuid::new_v4().simple().to_string();
        let secret = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let expires_at = now + session_ms;
        self.conn.execute(
            "DELETE FROM web_session WHERE expires_at < ?1 OR revoked_at < ?1",
            [now - WEB_SESSION_PRUNE_MS],
        )?;
        self.conn.execute(
            "INSERT INTO web_session (id, secret_hash, created_at, last_used_at, expires_at,
                                      identity, user_agent)
             VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6)",
            params![
                id,
                token_hash(&secret),
                now,
                expires_at,
                clip(identity),
                clip(user_agent)
            ],
        )?;
        Ok(IssuedWebSession {
            cookie: format!("{id}.{secret}"),
            id,
            expires_at,
        })
    }

    /// Sessão do valor do cookie, se válida (segredo certo, não revogada,
    /// dentro da validade e usada há menos de `idle_ms`); marca o uso.
    pub fn check_web_session(&self, cookie: &str, idle_ms: i64) -> Result<Option<HubWebSession>> {
        self.check_web_session_at(cookie, idle_ms, now_ms())
    }

    pub fn check_web_session_at(
        &self,
        cookie: &str,
        idle_ms: i64,
        now: i64,
    ) -> Result<Option<HubWebSession>> {
        let Some((id, secret)) = cookie.trim().split_once('.') else {
            return Ok(None);
        };
        if id.is_empty() || secret.is_empty() {
            return Ok(None);
        }
        let hash: Option<String> = self
            .conn
            .query_row(
                "SELECT secret_hash FROM web_session WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        match hash {
            Some(h) if eq_ct(&h, &token_hash(secret)) => {
                self.web_session_by_id_at(id, idle_ms, now)
            }
            _ => Ok(None),
        }
    }

    /// Sessão `id`, se ainda válida; marca o uso. Para a conexão do
    /// `/web/ws`, que já passou pelo cookie no upgrade.
    pub fn web_session_by_id(&self, id: &str, idle_ms: i64) -> Result<Option<HubWebSession>> {
        self.web_session_by_id_at(id, idle_ms, now_ms())
    }

    pub fn web_session_by_id_at(
        &self,
        id: &str,
        idle_ms: i64,
        now: i64,
    ) -> Result<Option<HubWebSession>> {
        let s = self
            .conn
            .query_row(
                &format!(
                    "SELECT {COLS} FROM web_session
                     WHERE id = ?1 AND revoked_at IS NULL AND expires_at > ?2
                       AND last_used_at > ?3"
                ),
                params![id, now, now - idle_ms],
                row_to_session,
            )
            .optional()?;
        let Some(mut s) = s else { return Ok(None) };
        if now - s.last_used_at >= WEB_SESSION_TOUCH_MS {
            self.conn.execute(
                "UPDATE web_session SET last_used_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
            s.last_used_at = now;
        }
        Ok(Some(s))
    }

    /// Sessões válidas, da mais recente para a mais antiga.
    pub fn list_web_sessions(&self, idle_ms: i64) -> Result<Vec<HubWebSession>> {
        self.list_web_sessions_at(idle_ms, now_ms())
    }

    pub fn list_web_sessions_at(&self, idle_ms: i64, now: i64) -> Result<Vec<HubWebSession>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLS} FROM web_session
             WHERE revoked_at IS NULL AND expires_at > ?1 AND last_used_at > ?2
             ORDER BY created_at DESC, id"
        ))?;
        let rows = stmt
            .query_map(params![now, now - idle_ms], row_to_session)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Revoga a sessão `id` e apaga as inscrições de Web Push dela (#203:
    /// o dispositivo perde o push junto com o acesso). `false` se não havia
    /// ou já estava revogada.
    pub fn revoke_web_session(&self, id: &str) -> Result<bool> {
        let tx = self.conn.unchecked_transaction()?;
        let n = self.conn.execute(
            "UPDATE web_session SET revoked_at = ?2 WHERE id = ?1 AND revoked_at IS NULL",
            params![id, now_ms()],
        )?;
        self.conn.execute(
            "DELETE FROM push_subscription WHERE web_session_id = ?1",
            [id],
        )?;
        tx.commit()?;
        Ok(n > 0)
    }

    /// Revoga todas as sessões web (inclusive as vencidas, por garantia) e
    /// apaga todas as inscrições de Web Push. Devolve quantas estavam ativas.
    pub fn revoke_all_web_sessions(&self) -> Result<usize> {
        let tx = self.conn.unchecked_transaction()?;
        let now = now_ms();
        let active: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM web_session WHERE revoked_at IS NULL AND expires_at > ?1",
            [now],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "UPDATE web_session SET revoked_at = ?1 WHERE revoked_at IS NULL",
            [now],
        )?;
        self.conn.execute("DELETE FROM push_subscription", [])?;
        tx.commit()?;
        Ok(active as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 24 * 3600 * 1000;
    const IDLE: i64 = 7 * DAY;

    #[test]
    fn cookie_valido_e_so_o_hash_guardado() {
        let hub = Hub::open_in_memory().unwrap();
        let s = hub
            .create_web_session_at(Some("eu@exemplo.com"), Some("Safari"), 30 * DAY, 1000)
            .unwrap();
        assert_eq!(s.expires_at, 1000 + 30 * DAY);
        let (id, secret) = s.cookie.split_once('.').unwrap();
        assert_eq!(id, s.id);
        assert!(secret.len() >= 64, "{secret}");
        let stored: String = hub
            .conn
            .query_row("SELECT secret_hash FROM web_session", [], |r| r.get(0))
            .unwrap();
        assert_ne!(stored, secret);
        assert_eq!(stored, token_hash(secret));

        let got = hub
            .check_web_session_at(&s.cookie, IDLE, 2000)
            .unwrap()
            .unwrap();
        assert_eq!(got.identity.as_deref(), Some("eu@exemplo.com"));
        assert_eq!(got.user_agent.as_deref(), Some("Safari"));
        for bad in [
            "",
            ".",
            "semponto",
            &format!("{id}."),
            &format!("{id}.errado"),
            &format!("outro.{secret}"),
            &format!("{id}.{secret}x"),
        ] {
            assert!(
                hub.check_web_session_at(bad, IDLE, 2000).unwrap().is_none(),
                "{bad}"
            );
        }
    }

    #[test]
    fn validade_absoluta_e_por_inatividade() {
        let hub = Hub::open_in_memory().unwrap();
        let t0 = 10 * DAY;
        let s = hub.create_web_session_at(None, None, 30 * DAY, t0).unwrap();
        let check = |t| hub.check_web_session_at(&s.cookie, IDLE, t).unwrap();
        // Sem uso por 7 dias: vencida.
        assert!(check(t0 + IDLE).is_none());
        // Usada a cada 6 dias, segue valendo até a validade absoluta.
        let mut t = t0;
        while t + 6 * DAY < t0 + 30 * DAY {
            t += 6 * DAY;
            let got = check(t).unwrap_or_else(|| panic!("em {t}"));
            assert_eq!(got.last_used_at, t);
        }
        assert!(check(t0 + 30 * DAY).is_none(), "validade absoluta");
        assert!(hub
            .list_web_sessions_at(IDLE, t0 + 30 * DAY)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn uso_gravado_no_maximo_uma_vez_por_minuto() {
        let hub = Hub::open_in_memory().unwrap();
        let s = hub.create_web_session_at(None, None, DAY, 0).unwrap();
        let got = hub
            .web_session_by_id_at(&s.id, IDLE, 59_000)
            .unwrap()
            .unwrap();
        assert_eq!(got.last_used_at, 0);
        let got = hub
            .web_session_by_id_at(&s.id, IDLE, 60_000)
            .unwrap()
            .unwrap();
        assert_eq!(got.last_used_at, 60_000);
    }

    #[test]
    fn revogacao_de_uma_e_de_todas() {
        let hub = Hub::open_in_memory().unwrap();
        let now = now_ms();
        let a = hub.create_web_session_at(None, None, DAY, now).unwrap();
        let b = hub.create_web_session_at(None, None, DAY, now + 1).unwrap();
        let c = hub.create_web_session_at(None, None, DAY, now + 2).unwrap();
        let ids: Vec<_> = hub
            .list_web_sessions(IDLE)
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, [c.id.clone(), b.id.clone(), a.id.clone()]);
        assert!(hub.revoke_web_session(&a.id).unwrap());
        assert!(!hub.revoke_web_session(&a.id).unwrap(), "já revogada");
        assert!(!hub.revoke_web_session("inventada").unwrap());
        assert!(hub.check_web_session(&a.cookie, IDLE).unwrap().is_none());
        assert!(hub.check_web_session(&b.cookie, IDLE).unwrap().is_some());
        assert_eq!(hub.revoke_all_web_sessions().unwrap(), 2);
        assert!(hub.check_web_session(&b.cookie, IDLE).unwrap().is_none());
        assert!(hub.check_web_session(&c.cookie, IDLE).unwrap().is_none());
        assert!(hub.list_web_sessions(IDLE).unwrap().is_empty());
        assert_eq!(hub.revoke_all_web_sessions().unwrap(), 0);
    }

    #[test]
    fn textos_cortados_e_vencidas_antigas_apagadas() {
        let hub = Hub::open_in_memory().unwrap();
        let ua = "x".repeat(500);
        let s = hub
            .create_web_session_at(Some("  "), Some(&ua), DAY, 0)
            .unwrap();
        let got = hub.web_session_by_id_at(&s.id, IDLE, 1).unwrap().unwrap();
        assert_eq!(got.identity, None);
        assert_eq!(got.user_agent.unwrap().len(), TEXT_MAX);
        // Uma sessão nova, bem depois, apaga a vencida há mais de 30 dias.
        hub.create_web_session_at(None, None, DAY, 40 * DAY)
            .unwrap();
        let n: i64 = hub
            .conn
            .query_row("SELECT COUNT(*) FROM web_session", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1);
    }
}
