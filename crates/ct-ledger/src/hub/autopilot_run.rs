//! Agendador do piloto automático no hub (#183, épico #178): o estado dele
//! (pausado, sessões autônomas por ambiente), os eventos que o painel
//! (#185) lê e as sessões estacionadas pelo limite real do plano
//! (migração 36). A decisão fica em `cthubd::autopilot`; aqui só se guarda.

use super::Hub;
use crate::{LedgerError, Result};
use ct_core::SessionState;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

/// Sessões autônomas simultâneas por ambiente, padrão do agendador.
pub const AUTOPILOT_MAX_PER_ENV_DEFAULT: u32 = 1;
/// Teto do ajuste de sessões autônomas por ambiente.
pub const AUTOPILOT_MAX_PER_ENV_CEILING: u32 = 8;
/// Eventos do agendador mais velhos que isto são apagados ao gravar.
pub const AUTOPILOT_EVENT_TTL_MS: i64 = 90 * 86_400_000;
/// Minutos que uma sessão autônoma pode ficar ociosa sem PR antes de o
/// agendador a encerrar, padrão.
pub const AUTOPILOT_IDLE_TIMEOUT_DEFAULT_MIN: u32 = 30;
/// Teto do ajuste acima (um dia).
pub const AUTOPILOT_IDLE_TIMEOUT_MAX_MIN: u32 = 24 * 60;

/// Estado do agendador do piloto automático (`hub.autopilot.status`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AutopilotSettings {
    /// "Pausar tudo": o agendador não inicia nada; as sessões em andamento
    /// continuam.
    pub paused: bool,
    pub paused_at: Option<i64>,
    /// Sessões autônomas vivas ao mesmo tempo em cada ambiente.
    pub max_sessions_per_env: u32,
    /// Minutos que uma sessão autônoma pode ficar ociosa sem PR antes de o
    /// agendador a encerrar (`sessao_ociosa`); 0 = nunca. A ociosa com PR
    /// aberto encerra sempre.
    pub idle_timeout_min: u32,
}

impl Default for AutopilotSettings {
    fn default() -> Self {
        AutopilotSettings {
            paused: false,
            paused_at: None,
            max_sessions_per_env: AUTOPILOT_MAX_PER_ENV_DEFAULT,
            idle_timeout_min: AUTOPILOT_IDLE_TIMEOUT_DEFAULT_MIN,
        }
    }
}

/// Um evento do agendador: início, recusa relevante, estacionamento,
/// retomada, pausa. Lido pelo painel (#185) e pelo resumo da manhã.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AutopilotEvent {
    /// Ignorado ao gravar.
    pub id: i64,
    pub at: i64,
    /// `started`, `start_failed`, `refused`, `parked`, `resumed`,
    /// `resume_failed`, `unparked`, `ended`, `end_failed`, `paused` ou
    /// `unpaused`.
    pub kind: String,
    /// De quem é o evento: `hub`, `account:<chave>`, `project:<owner/repo>`
    /// ou `item:<owner/repo#n>`. A recusa (`refused`, `start_failed`,
    /// `end_failed`) só é gravada de novo quando o motivo (`reason`) ou a
    /// causa no texto (`message`, sem os números) muda no mesmo escopo.
    pub scope: String,
    /// Código estável do motivo (ex.: `budget:band_closed`, `wip_limit`).
    pub reason: Option<String>,
    /// Texto para a pessoa (pt-BR), sem nada da conversa do agente.
    pub message: String,
    pub account_key: Option<String>,
    pub env_id: Option<String>,
    pub owner: Option<String>,
    pub repo: Option<String>,
    pub gh_number: Option<i64>,
    pub session_id: Option<String>,
}

/// Sessão estacionada pelo limite real do plano: o agendador a retoma
/// depois de `resume_at` (o `resets_at` da janela esgotada).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct ParkedSession {
    pub session_id: String,
    pub env_id: String,
    pub account_key: String,
    pub owner: String,
    pub repo: String,
    pub gh_number: i64,
    /// Janela esgotada: `five_hour` ou `seven_day`.
    pub window: String,
    pub resume_at: i64,
    pub parked_at: i64,
}

/// Sessão ociosa (`idle`) como o agendador a vê para decidir encerrá-la.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdleSession {
    pub session_id: String,
    /// Desde quando está ociosa: o `ts` do último `session_state` para
    /// `idle` (o `Stop` que a deixou ociosa); sem ele no hub, o
    /// `last_event_at`. O aviso de espera do Claude Code depois do `Stop`
    /// (hook `Notification`) avança o `last_event_at` mas não é atividade:
    /// não adia o encerramento.
    pub since: i64,
    /// Sessão autônoma: iniciada pelo agendador ou depois de um início
    /// autônomo da issue no mesmo ambiente (evento `autonomy_session`).
    pub autonomous: bool,
    /// A issue já tem PR aberto ou está em `em_revisao` ou `concluido`.
    pub done: bool,
}

/// Sessão de implementação como o hub a conhece, para o agendador.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct AutopilotSession {
    pub session_id: String,
    pub env_id: String,
    pub state: SessionState,
    pub owner: String,
    pub repo: String,
    pub gh_number: i64,
    pub started_at: i64,
}

/// A causa no texto de uma recusa, para a deduplicação: os números
/// (percentuais, contagens, horários) viram `#`.
fn message_cause(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    let mut in_number = false;
    for c in message.chars() {
        if c.is_ascii_digit() {
            if !in_number {
                out.push('#');
            }
            in_number = true;
        } else if in_number && matches!(c, '.' | ',') {
            // Separador decimal dentro do número.
        } else {
            in_number = false;
            out.push(c);
        }
    }
    out
}

const EVENT_COLS: &str =
    "id, at, kind, scope, reason, message, account_key, env_id, owner, repo, gh_number, session_id";
const PARKED_COLS: &str =
    "session_id, env_id, account_key, owner, repo, gh_number, window, resume_at, parked_at";

fn row_to_event(r: &rusqlite::Row) -> rusqlite::Result<AutopilotEvent> {
    Ok(AutopilotEvent {
        id: r.get(0)?,
        at: r.get(1)?,
        kind: r.get(2)?,
        scope: r.get(3)?,
        reason: r.get(4)?,
        message: r.get(5)?,
        account_key: r.get(6)?,
        env_id: r.get(7)?,
        owner: r.get(8)?,
        repo: r.get(9)?,
        gh_number: r.get(10)?,
        session_id: r.get(11)?,
    })
}

fn row_to_parked(r: &rusqlite::Row) -> rusqlite::Result<ParkedSession> {
    Ok(ParkedSession {
        session_id: r.get(0)?,
        env_id: r.get(1)?,
        account_key: r.get(2)?,
        owner: r.get(3)?,
        repo: r.get(4)?,
        gh_number: r.get(5)?,
        window: r.get(6)?,
        resume_at: r.get(7)?,
        parked_at: r.get(8)?,
    })
}

impl Hub {
    /// Estado do agendador; sem linha = o padrão (ligado, 1 por ambiente).
    pub fn autopilot_settings(&self) -> Result<AutopilotSettings> {
        Ok(self
            .conn
            .query_row(
                "SELECT paused, paused_at, max_sessions_per_env, idle_timeout_min
                 FROM autopilot_state WHERE id = 1",
                [],
                |r| {
                    Ok(AutopilotSettings {
                        paused: r.get(0)?,
                        paused_at: r.get(1)?,
                        max_sessions_per_env: r.get::<_, i64>(2)?.max(1) as u32,
                        idle_timeout_min: r.get::<_, i64>(3)?.clamp(0, u32::MAX as i64) as u32,
                    })
                },
            )
            .optional()?
            .unwrap_or_default())
    }

    fn save_autopilot_settings(&self, s: &AutopilotSettings, now: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO autopilot_state (id, paused, paused_at, max_sessions_per_env,
                                          idle_timeout_min, updated_at)
             VALUES (1, ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (id) DO UPDATE SET paused = excluded.paused,
               paused_at = excluded.paused_at,
               max_sessions_per_env = excluded.max_sessions_per_env,
               idle_timeout_min = excluded.idle_timeout_min,
               updated_at = excluded.updated_at",
            params![
                s.paused,
                s.paused_at,
                s.max_sessions_per_env,
                s.idle_timeout_min,
                now
            ],
        )?;
        Ok(())
    }

    /// Pausa ou retoma o agendador (`hub.autopilot.pause|resume`). Devolve
    /// o estado novo e se mudou (pausar o pausado não muda nada).
    pub fn set_autopilot_paused(
        &self,
        paused: bool,
        now: i64,
    ) -> Result<(AutopilotSettings, bool)> {
        let mut s = self.autopilot_settings()?;
        if s.paused == paused {
            return Ok((s, false));
        }
        s.paused = paused;
        s.paused_at = paused.then_some(now);
        self.save_autopilot_settings(&s, now)?;
        Ok((s, true))
    }

    /// Sessões autônomas simultâneas por ambiente (1 a
    /// [`AUTOPILOT_MAX_PER_ENV_CEILING`]).
    pub fn set_autopilot_max_per_env(&self, n: u32, now: i64) -> Result<AutopilotSettings> {
        if !(1..=AUTOPILOT_MAX_PER_ENV_CEILING).contains(&n) {
            return Err(LedgerError::Conflict(format!(
                "sessões autônomas por ambiente: de 1 a {AUTOPILOT_MAX_PER_ENV_CEILING}"
            )));
        }
        let mut s = self.autopilot_settings()?;
        s.max_sessions_per_env = n;
        self.save_autopilot_settings(&s, now)?;
        Ok(s)
    }

    /// Minutos de ociosidade sem PR antes de encerrar a sessão autônoma
    /// (0 a [`AUTOPILOT_IDLE_TIMEOUT_MAX_MIN`]; 0 = nunca).
    pub fn set_autopilot_idle_timeout(&self, min: u32, now: i64) -> Result<AutopilotSettings> {
        if min > AUTOPILOT_IDLE_TIMEOUT_MAX_MIN {
            return Err(LedgerError::Conflict(format!(
                "minutos de sessão ociosa: de 0 a {AUTOPILOT_IDLE_TIMEOUT_MAX_MIN}"
            )));
        }
        let mut s = self.autopilot_settings()?;
        s.idle_timeout_min = min;
        self.save_autopilot_settings(&s, now)?;
        Ok(s)
    }

    /// Último evento do escopo.
    fn last_autopilot_event(&self, scope: &str) -> Result<Option<AutopilotEvent>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {EVENT_COLS} FROM autopilot_event WHERE scope = ?1
                     ORDER BY id DESC LIMIT 1"
                ),
                [scope],
                row_to_event,
            )
            .optional()?)
    }

    /// Grava o evento. Uma recusa (`refused`, `start_failed` ou
    /// `end_failed`) igual ao último evento do mesmo escopo não é gravada de
    /// novo: o agendador roda a cada minuto e só registra quando a causa
    /// muda. Igual = mesmo `reason` e mesma causa no texto: o `message` sem
    /// os números ([`message_cause`]), para o limite de trabalho em
    /// andamento que passa de sessões a PRs abertos virar evento, mas o uso
    /// do plano que sobe 1% não. Devolve o evento gravado, ou `None` se
    /// repetido. Apaga os eventos mais velhos que [`AUTOPILOT_EVENT_TTL_MS`].
    pub fn record_autopilot_event(&self, e: &AutopilotEvent) -> Result<Option<AutopilotEvent>> {
        if matches!(e.kind.as_str(), "refused" | "start_failed" | "end_failed") {
            if let Some(last) = self.last_autopilot_event(&e.scope)? {
                if last.kind == e.kind
                    && last.reason == e.reason
                    && message_cause(&last.message) == message_cause(&e.message)
                {
                    return Ok(None);
                }
            }
        }
        self.conn.execute(
            "DELETE FROM autopilot_event WHERE at < ?1",
            [e.at - AUTOPILOT_EVENT_TTL_MS],
        )?;
        self.conn.execute(
            "INSERT INTO autopilot_event (at, kind, scope, reason, message, account_key, env_id,
                                          owner, repo, gh_number, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                e.at,
                e.kind,
                e.scope,
                e.reason,
                e.message,
                e.account_key,
                e.env_id,
                e.owner,
                e.repo,
                e.gh_number,
                e.session_id
            ],
        )?;
        let mut saved = e.clone();
        saved.id = self.conn.last_insert_rowid();
        Ok(Some(saved))
    }

    /// Último evento de cada escopo que começa com `prefix`, para o
    /// estado atual de cada projeto ou conta no painel.
    pub fn last_autopilot_events(&self, prefix: &str) -> Result<Vec<AutopilotEvent>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {EVENT_COLS} FROM autopilot_event
             WHERE id IN (SELECT max(id) FROM autopilot_event
                          WHERE substr(scope, 1, length(?1)) = ?1 GROUP BY scope)
             ORDER BY scope"
        ))?;
        let rows = stmt.query_map([prefix], row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Eventos do agendador, do mais novo para o mais velho; `before` =
    /// só os de id menor (paginação).
    pub fn list_autopilot_events(
        &self,
        limit: u32,
        before: Option<i64>,
    ) -> Result<Vec<AutopilotEvent>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {EVENT_COLS} FROM autopilot_event WHERE (?1 IS NULL OR id < ?1)
             ORDER BY id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![before, limit.clamp(1, 500)], row_to_event)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Inícios do agendador desde `since` cuja sessão o hub ainda não
    /// conhece (o ambiente não sincronizou): `(env_id, owner, repo, n)`.
    /// Contam como vivos, para o ciclo seguinte não iniciar outra sessão no
    /// mesmo ambiente nem a mesma issue.
    pub fn autopilot_pending_starts(
        &self,
        since: i64,
    ) -> Result<Vec<(String, String, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT env_id, owner, repo, gh_number FROM autopilot_event e
             WHERE kind = 'started' AND at >= ?1 AND env_id IS NOT NULL AND owner IS NOT NULL
               AND repo IS NOT NULL AND gh_number IS NOT NULL
               AND (session_id IS NULL OR NOT EXISTS (SELECT 1 FROM session s
                                                      WHERE s.id = e.session_id))
             ORDER BY id",
        )?;
        let rows = stmt.query_map([since], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Sessões de implementação (`gh_number > 0`) vivas de todos os
    /// ambientes, mais as estacionadas em qualquer estado.
    pub fn autopilot_sessions(&self) -> Result<Vec<AutopilotSession>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.env_id, s.state, p.gh_owner, p.gh_repo, w.gh_number, s.started_at
             FROM session s
             JOIN work_item w ON w.id = s.work_item_id
             JOIN project p ON p.id = w.project_id
             WHERE w.gh_number > 0
               AND (s.state IN ('starting', 'running', 'idle')
                    OR s.id IN (SELECT session_id FROM autopilot_parked))
             ORDER BY s.started_at, s.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(AutopilotSession {
                session_id: r.get(0)?,
                env_id: r.get(1)?,
                state: crate::parse_col::<SessionState>(r, 2)?,
                owner: r.get(3)?,
                repo: r.get(4)?,
                gh_number: r.get(5)?,
                started_at: r.get(6)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Sessões de implementação ociosas, com o que o agendador precisa para
    /// encerrar as autônomas que terminaram ou pararam ([`IdleSession`]).
    pub fn autopilot_idle_sessions(&self) -> Result<Vec<IdleSession>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.id,
                    coalesce((SELECT max(e.ts) FROM event e
                              WHERE e.work_item_id = s.work_item_id AND e.session_id = s.id
                                AND e.type = 'session_state'
                                AND json_extract(e.payload, '$.to') = 'idle'),
                             s.last_event_at),
                    EXISTS (SELECT 1 FROM autopilot_event a
                            WHERE a.kind = 'started' AND a.session_id = s.id)
                    OR EXISTS (SELECT 1 FROM event e
                               WHERE e.work_item_id = s.work_item_id
                                 AND e.env_id = s.env_id AND e.type = 'autonomy_session'
                                 AND e.ts <= s.started_at),
                    w.status IN ('em_revisao', 'concluido')
                    OR EXISTS (SELECT 1 FROM pr_cache c
                               WHERE c.work_item_id = w.id AND c.state = 'open')
             FROM session s
             JOIN work_item w ON w.id = s.work_item_id
             WHERE w.gh_number > 0 AND s.state = 'idle'
             ORDER BY s.started_at, s.id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(IdleSession {
                session_id: r.get(0)?,
                since: r.get(1)?,
                autonomous: r.get(2)?,
                done: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Sessões que o agendador encerrou (ou tentou encerrar) desde `since`
    /// (eventos `ended` e `end_failed`): até o ambiente sincronizar, o hub
    /// ainda as vê ociosas; a que falhou espera antes de tentar de novo.
    pub fn autopilot_recent_ends(&self, since: i64) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT session_id FROM autopilot_event
             WHERE kind IN ('ended', 'end_failed') AND at >= ?1 AND session_id IS NOT NULL",
        )?;
        let rows = stmt.query_map([since], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Estaciona a sessão (substitui um estacionamento anterior dela).
    pub fn park_session(&self, p: &ParkedSession) -> Result<()> {
        self.conn.execute(
            &format!(
                "INSERT OR REPLACE INTO autopilot_parked ({PARKED_COLS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
            ),
            params![
                p.session_id,
                p.env_id,
                p.account_key,
                p.owner,
                p.repo,
                p.gh_number,
                p.window,
                p.resume_at,
                p.parked_at
            ],
        )?;
        Ok(())
    }

    /// Tira a sessão do estacionamento; `false` se não estava.
    pub fn unpark_session(&self, session_id: &str) -> Result<bool> {
        Ok(self.conn.execute(
            "DELETE FROM autopilot_parked WHERE session_id = ?1",
            [session_id],
        )? > 0)
    }

    /// Sessões estacionadas, pela ordem de retomada.
    pub fn list_parked(&self) -> Result<Vec<ParkedSession>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {PARKED_COLS} FROM autopilot_parked ORDER BY resume_at, parked_at"
        ))?;
        let rows = stmt.query_map([], row_to_parked)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NewHubProject;

    fn hub() -> Hub {
        let hub = Hub::open_in_memory().unwrap();
        hub.ingest("env-auto", "env-auto", &[]).unwrap();
        hub.add_project(&NewHubProject {
            owner: "acme".into(),
            repo: "acme-web".into(),
            name: "acme-web".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        hub
    }

    fn refused(reason: &str, at: i64) -> AutopilotEvent {
        AutopilotEvent {
            at,
            kind: "refused".into(),
            scope: "account:conta-a".into(),
            reason: Some(reason.into()),
            message: format!("recusado: {reason}"),
            account_key: Some("conta-a".into()),
            ..Default::default()
        }
    }

    #[test]
    fn pausa_persiste_e_limite_por_ambiente() {
        let h = hub();
        assert_eq!(
            h.autopilot_settings().unwrap(),
            AutopilotSettings::default()
        );
        let (s, changed) = h.set_autopilot_paused(true, 10).unwrap();
        assert!(changed && s.paused);
        assert_eq!(s.paused_at, Some(10));
        let (_, changed) = h.set_autopilot_paused(true, 20).unwrap();
        assert!(!changed);
        assert_eq!(h.autopilot_settings().unwrap().paused_at, Some(10));
        assert_eq!(
            h.set_autopilot_max_per_env(2, 30)
                .unwrap()
                .max_sessions_per_env,
            2
        );
        assert!(h.set_autopilot_max_per_env(0, 30).is_err());
        assert!(h.set_autopilot_max_per_env(9, 30).is_err());
        let (s, _) = h.set_autopilot_paused(false, 40).unwrap();
        assert!(!s.paused && s.paused_at.is_none());
        assert_eq!(s.max_sessions_per_env, 2);
        assert_eq!(s.idle_timeout_min, AUTOPILOT_IDLE_TIMEOUT_DEFAULT_MIN);
        let s = h.set_autopilot_idle_timeout(0, 50).unwrap();
        assert_eq!((s.idle_timeout_min, s.max_sessions_per_env), (0, 2));
        assert!(h
            .set_autopilot_idle_timeout(AUTOPILOT_IDLE_TIMEOUT_MAX_MIN + 1, 50)
            .is_err());
        assert_eq!(h.autopilot_settings().unwrap().idle_timeout_min, 0);
    }

    #[test]
    fn recusa_grava_de_novo_quando_a_causa_no_texto_muda() {
        let h = hub();
        let wip = |m: &str, at| AutopilotEvent {
            scope: "project:acme/acme-web".into(),
            message: format!("acme/acme-web: limite de trabalho em andamento ({m})"),
            ..refused("wip_limit", at)
        };
        assert!(h
            .record_autopilot_event(&wip("2 de 2 sessões", 1))
            .unwrap()
            .is_some());
        assert!(h
            .record_autopilot_event(&wip("2 de 2 sessões", 2))
            .unwrap()
            .is_none());
        // Mesmo `reason`, outra causa: grava.
        assert!(h
            .record_autopilot_event(&wip("3 de 3 PRs abertos", 3))
            .unwrap()
            .is_some());
        // Só os números mudaram (o limite subiu, o uso do plano andou): não.
        assert!(h
            .record_autopilot_event(&wip("4 de 4 PRs abertos", 4))
            .unwrap()
            .is_none());
        let budget = |m: &str, at| AutopilotEvent {
            message: format!("a política do plano não permite iniciar agora: {m}"),
            ..refused("budget:weekly_over_limit", at)
        };
        assert!(h
            .record_autopilot_event(&budget("semanal em 41%, limite 40.2%", 5))
            .unwrap()
            .is_some());
        assert!(h
            .record_autopilot_event(&budget("semanal em 42%, limite 40.7%", 6))
            .unwrap()
            .is_none());
        assert_eq!(message_cause("5h em 41.5%, teto 60%"), "#h em #%, teto #%");
    }

    #[test]
    fn sessoes_ociosas_com_autonomia_e_pr() {
        let h = hub();
        h.conn
            .execute(
                "INSERT INTO work_item (project_id, gh_number, created_at)
                 SELECT id, 7, 1 FROM project",
                [],
            )
            .unwrap();
        let wi = h.conn.last_insert_rowid();
        let insert = |id: &str, started: i64, state: &str| {
            h.conn
                .execute(
                    "INSERT INTO session (id, env_id, work_item_id, tool, cwd, state, started_at,
                                          last_event_at)
                     VALUES (?1, 'env-auto', ?2, 'claude', '/tmp/x', ?3, ?4, ?4 + 5)",
                    params![id, wi, state, started],
                )
                .unwrap();
        };
        insert("antes", 100, "idle");
        insert("depois", 300, "idle");
        insert("rodando", 300, "running");
        h.conn
            .execute(
                "INSERT INTO event (uid, env_id, hlc, ts, project_id, work_item_id, type, payload)
                 SELECT 'u1', 'env-auto', 1, 200, project_id, id, 'autonomy_session', '{}'
                 FROM work_item WHERE id = ?1",
                [wi],
            )
            .unwrap();
        let idle = h.autopilot_idle_sessions().unwrap();
        let view: Vec<_> = idle
            .iter()
            .map(|i| (i.session_id.as_str(), i.since, i.autonomous, i.done))
            .collect();
        assert_eq!(
            view,
            vec![("antes", 105, false, false), ("depois", 305, true, false)]
        );
        // O `Stop` em 302 e o aviso de espera do Claude Code em 305 (que
        // avançou o `last_event_at`): ociosa desde o `Stop` (R3-1, #178).
        h.conn
            .execute(
                "INSERT INTO event (uid, env_id, hlc, ts, project_id, work_item_id, session_id,
                                    type, payload)
                 SELECT 'u2', 'env-auto', 2, 302, project_id, id, 'depois', 'session_state',
                        '{\"from\":\"running\",\"to\":\"idle\"}'
                 FROM work_item WHERE id = ?1",
                [wi],
            )
            .unwrap();
        let since = |id: &str| {
            h.autopilot_idle_sessions()
                .unwrap()
                .into_iter()
                .find(|i| i.session_id == id)
                .unwrap()
                .since
        };
        assert_eq!(since("depois"), 302);
        assert_eq!(since("antes"), 105);
        // PR aberto da issue: terminou.
        h.conn
            .execute(
                "INSERT INTO pr_cache (project_id, number, work_item_id, head, base, state, url,
                                       updated_at, synced_at)
                 SELECT project_id, 9, id, 'feat/7', 'wip/autonoma', 'open', '', 1, 1
                 FROM work_item WHERE id = ?1",
                [wi],
            )
            .unwrap();
        assert!(h.autopilot_idle_sessions().unwrap().iter().all(|i| i.done));
        // Iniciada pelo agendador: autônoma mesmo sem o evento do ambiente.
        h.record_autopilot_event(&AutopilotEvent {
            at: 1,
            kind: "started".into(),
            scope: "project:acme/acme-web".into(),
            session_id: Some("antes".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(h
            .autopilot_idle_sessions()
            .unwrap()
            .iter()
            .all(|i| i.autonomous));

        assert!(h.autopilot_recent_ends(0).unwrap().is_empty());
        for (kind, sid, at) in [
            ("ended", "a", 10),
            ("end_failed", "b", 20),
            ("ended", "c", 5),
        ] {
            h.record_autopilot_event(&AutopilotEvent {
                at,
                kind: kind.into(),
                scope: format!("item:{sid}"),
                session_id: Some(sid.into()),
                ..Default::default()
            })
            .unwrap();
        }
        let mut ends = h.autopilot_recent_ends(10).unwrap();
        ends.sort();
        assert_eq!(ends, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn recusa_repetida_so_grava_quando_o_motivo_muda() {
        let h = hub();
        assert!(h
            .record_autopilot_event(&refused("budget:band_closed", 1))
            .unwrap()
            .is_some());
        assert!(h
            .record_autopilot_event(&refused("budget:band_closed", 2))
            .unwrap()
            .is_none());
        assert!(h
            .record_autopilot_event(&refused("budget:after_cutoff", 3))
            .unwrap()
            .is_some());
        // Outro evento no escopo: a mesma recusa volta a ser gravada.
        let started = AutopilotEvent {
            kind: "started".into(),
            ..refused("x", 4)
        };
        h.record_autopilot_event(&started).unwrap().unwrap();
        assert!(h
            .record_autopilot_event(&refused("budget:after_cutoff", 5))
            .unwrap()
            .is_some());
        let all = h.list_autopilot_events(50, None).unwrap();
        assert_eq!(all.len(), 4);
        assert_eq!(all[0].at, 5);
        let older = h.list_autopilot_events(50, Some(all[1].id)).unwrap();
        assert_eq!(older.len(), 2);
        let last = h.last_autopilot_events("account:").unwrap();
        assert_eq!(last.len(), 1);
        assert_eq!(last[0].at, 5);
        // Os velhos saem ao gravar.
        h.record_autopilot_event(&refused("budget:no_band", AUTOPILOT_EVENT_TTL_MS + 5))
            .unwrap();
        assert_eq!(h.list_autopilot_events(50, None).unwrap().len(), 2);
    }

    #[test]
    fn estacionar_e_inicios_pendentes() {
        let h = hub();
        let p = ParkedSession {
            session_id: "s1".into(),
            env_id: "env-auto".into(),
            account_key: "conta-a".into(),
            owner: "acme".into(),
            repo: "acme-web".into(),
            gh_number: 123,
            window: "five_hour".into(),
            resume_at: 100,
            parked_at: 50,
        };
        h.park_session(&p).unwrap();
        assert_eq!(h.list_parked().unwrap(), vec![p.clone()]);
        assert!(h.unpark_session("s1").unwrap());
        assert!(!h.unpark_session("s1").unwrap());

        let started = AutopilotEvent {
            at: 1000,
            kind: "started".into(),
            scope: "project:acme/acme-web".into(),
            message: "iniciada".into(),
            env_id: Some("env-auto".into()),
            owner: Some("acme".into()),
            repo: Some("acme-web".into()),
            gh_number: Some(123),
            session_id: Some("s9".into()),
            ..Default::default()
        };
        h.record_autopilot_event(&started).unwrap();
        assert_eq!(
            h.autopilot_pending_starts(500).unwrap(),
            vec![("env-auto".into(), "acme".into(), "acme-web".into(), 123)]
        );
        assert!(h.autopilot_pending_starts(2000).unwrap().is_empty());
        assert!(h.autopilot_sessions().unwrap().is_empty());
    }
}
