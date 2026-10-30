//! Web Push da PWA (issue #203, spec da PWA §7): as inscrições por
//! dispositivo (`push_subscription`), presas à sessão web que as criou, e a
//! deduplicação do que já foi notificado (`push_sent`).
//!
//! O hub decide o que notificar a partir do que o sync já trouxe
//! ([`Hub::push_collect`]): sessão que saiu com erro, ambiente sem sync,
//! proposta nova do gestor e "aguardando você" de sessão viva **sem** Remote
//! Control (a com Remote Control já recebe o push nativo do app Claude). O
//! envio cifrado (RFC 8291) e a chave VAPID ficam no `cthubd`.

use super::Hub;
use crate::{LedgerError, Result};
use ct_core::now_ms;
use rusqlite::{params, OptionalExtension, Row};
use serde::{Deserialize, Serialize};

/// Falhas seguidas (fora 404/410) que desativam a inscrição (spec §7.2).
pub const PUSH_FAILURES_MAX: i64 = 10;
/// Teto de inscrições guardadas: o hub tem um usuário só.
pub const PUSH_SUBSCRIPTIONS_MAX: i64 = 50;
/// Chaves de deduplicação mais velhas que isso saem de `push_sent`.
const PUSH_SENT_KEEP_MS: i64 = 30 * 24 * 3600 * 1000;
/// Marca da primeira varredura: o que já existia antes dela não vira push.
const BASELINE_KEY: &str = "_baseline";
const ENDPOINT_MAX: usize = 2048;
const LABEL_MAX: usize = 80;
const TITLE_MAX: usize = 120;

/// Canal reverso fechado há mais que isso vira `env_stale` (spec da PWA
/// §7.1, fase 2).
pub const CHANNEL_STALE_MS: i64 = 60_000;

/// Estado do canal reverso por `env_id` (fase 2): `None` = canal aberto
/// agora; `Some(t)` = fechado desde `t` (ms). Ambiente fora do mapa (nunca
/// abriu canal desde que o hub subiu, como um `ctowerd` antigo) segue pela
/// idade do último sync.
pub type ChannelStates = std::collections::HashMap<String, Option<i64>>;

/// Evento que um dispositivo pode receber (spec §7.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum PushEvent {
    /// Uma sessão saiu com código ≠ 0.
    SessionError,
    /// Um ambiente está sem sync há mais de `env_stale_minutes`.
    EnvStale,
    /// Proposta nova do gestor.
    ProposalNew,
    /// "Aguardando você" de sessão viva sem Remote Control.
    NeedsHuman,
}

impl PushEvent {
    pub const ALL: [PushEvent; 4] = [
        PushEvent::SessionError,
        PushEvent::EnvStale,
        PushEvent::ProposalNew,
        PushEvent::NeedsHuman,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            PushEvent::SessionError => "session_error",
            PushEvent::EnvStale => "env_stale",
            PushEvent::ProposalNew => "proposal_new",
            PushEvent::NeedsHuman => "needs_human",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.as_str() == s)
    }
}

/// Dispositivo inscrito em `hub.push.list` (sem o endpoint nem as chaves).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct HubPushDevice {
    pub id: i64,
    /// Nome dado pelo navegador na inscrição (ex.: "Safari · iPhone").
    pub label: Option<String>,
    /// Eventos que este dispositivo recebe.
    pub events: Vec<PushEvent>,
    /// Host do serviço de push (ex.: `fcm.googleapis.com`, `web.push.apple.com`).
    pub service: String,
    pub created_at: i64,
    /// Última entrega aceita pelo serviço de push.
    pub last_ok_at: Option<i64>,
    /// Falhas seguidas (fora 404/410).
    pub failures: i64,
    /// Desativada depois de [`PUSH_FAILURES_MAX`] falhas seguidas.
    pub disabled_at: Option<i64>,
    /// Inscrição da sessão web da própria conexão (só no `/web/ws`).
    #[serde(default)]
    pub current: bool,
}

/// `hub.push.subscribe`: o `PushSubscription` do navegador.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NewPushSubscription {
    pub endpoint: String,
    /// Chave pública P-256 do navegador (base64url).
    pub p256dh: String,
    /// Segredo de autenticação (base64url).
    pub auth: String,
    pub label: Option<String>,
    /// `None` = mantém os da inscrição existente ou, nova, todos.
    pub events: Option<Vec<PushEvent>>,
}

/// Para onde enviar: só no hub, nunca sai pelo `/ws`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushTarget {
    pub id: i64,
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

/// Notificação a enviar: o corpo leva só projeto, número, título e o tipo
/// do evento (spec §7.1), nunca mensagem do agente nem comando.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PushNotice {
    /// Chave de deduplicação (`session_error:<sessão>`, ...).
    #[serde(skip)]
    pub key: String,
    pub event: PushEvent,
    pub title: String,
    pub body: String,
    /// Deep link (spec §7.3): `https://claude.ai/code/<id>` ou `/?go=...`.
    pub url: String,
    /// Agrupa no aparelho: uma notificação nova do mesmo assunto substitui a anterior.
    pub tag: String,
}

/// Deep link de um item (spec §7.3).
pub fn item_url(owner: &str, repo: &str, n: i64) -> String {
    format!("/?go=item&project={owner}/{repo}&n={n}")
}

/// Deep link de uma sessão: o Remote Control se ele está ligado (sessão viva
/// com `rc_session_id`), senão o item na PWA.
fn session_url(s: &SessionRow) -> String {
    match (&s.rc_session_id, s.live) {
        (Some(rc), true) if !rc.trim().is_empty() => {
            format!("https://claude.ai/code/{}", rc.trim())
        }
        _ if s.gestor => "/".into(),
        _ => item_url(&s.owner, &s.repo, s.number),
    }
}

fn clip(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// Corpo da notificação de um item: `owner/repo #n · título`, ou
/// `CHAVE-n · título` no projeto Jira (`jira_key`, épico #353).
fn item_body(
    owner: &str,
    repo: &str,
    n: i64,
    title: Option<&str>,
    gestor: bool,
    jira_key: Option<&str>,
) -> String {
    if gestor {
        return "Gestor de projetos".into();
    }
    let item = match jira_key.filter(|k| !k.is_empty()) {
        Some(k) => format!("{k}-{n}"),
        None => format!("{owner}/{repo} #{n}"),
    };
    match title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(t) => format!("{item} · {}", clip(t, TITLE_MAX)),
        None => item,
    }
}

/// Chave do projeto Jira (`tracker_config.project_key`), nula no GitHub.
const JIRA_KEY_COL: &str =
    "CASE WHEN p.tracker_kind = 'jira' THEN json_extract(p.tracker_config, '$.project_key') END";

/// Sessão de um candidato, com o item.
struct SessionRow {
    id: String,
    owner: String,
    repo: String,
    number: i64,
    title: Option<String>,
    gestor: bool,
    rc_session_id: Option<String>,
    live: bool,
    jira_key: Option<String>,
}

const SESSION_JOIN: &str = "SELECT s.id, p.gh_owner, p.gh_repo, w.gh_number, ic.title,
            p.kind = 'gestor', s.rc_session_id, s.state IN ('starting', 'running', 'idle'),
            CASE WHEN p.tracker_kind = 'jira'
                 THEN json_extract(p.tracker_config, '$.project_key') END
     FROM session s
     JOIN work_item w ON w.id = s.work_item_id
     JOIN project p ON p.id = w.project_id
     LEFT JOIN issue_cache ic ON ic.project_id = p.id AND ic.gh_number = w.gh_number";

fn row_to_session(r: &Row, off: usize) -> rusqlite::Result<SessionRow> {
    Ok(SessionRow {
        id: r.get(off)?,
        owner: r.get(off + 1)?,
        repo: r.get(off + 2)?,
        number: r.get(off + 3)?,
        title: r.get(off + 4)?,
        gestor: r.get(off + 5)?,
        rc_session_id: r.get(off + 6)?,
        live: r.get(off + 7)?,
        jira_key: r.get(off + 8)?,
    })
}

const DEVICE_COLS: &str = "id, label, events, endpoint, created_at, last_ok_at, failures, \
                           disabled_at, web_session_id";

fn service_of(endpoint: &str) -> String {
    let rest = endpoint.split_once("://").map_or(endpoint, |(_, r)| r);
    rest.split(['/', '?']).next().unwrap_or("").to_string()
}

fn row_to_device(r: &Row, current: Option<&str>) -> rusqlite::Result<HubPushDevice> {
    let events: String = r.get(2)?;
    let endpoint: String = r.get(3)?;
    let session: Option<String> = r.get(8)?;
    Ok(HubPushDevice {
        id: r.get(0)?,
        label: r.get(1)?,
        events: parse_events(&events),
        service: service_of(&endpoint),
        created_at: r.get(4)?,
        last_ok_at: r.get(5)?,
        failures: r.get(6)?,
        disabled_at: r.get(7)?,
        current: current.is_some() && session.as_deref() == current,
    })
}

fn parse_events(s: &str) -> Vec<PushEvent> {
    let v: Vec<String> = serde_json::from_str(s).unwrap_or_default();
    let mut out: Vec<PushEvent> = v.iter().filter_map(|e| PushEvent::parse(e)).collect();
    out.sort();
    out.dedup();
    out
}

fn events_json(events: &[PushEvent]) -> String {
    let mut v: Vec<PushEvent> = events.to_vec();
    v.sort();
    v.dedup();
    serde_json::to_string(&v.iter().map(|e| e.as_str()).collect::<Vec<_>>())
        .expect("lista de strings serializa")
}

impl Hub {
    /// Grava ou atualiza (pelo endpoint) a inscrição deste dispositivo,
    /// presa à sessão web `web_session_id`. Reinscrever reativa e zera as
    /// falhas.
    pub fn subscribe_push(
        &self,
        web_session_id: &str,
        s: &NewPushSubscription,
    ) -> Result<HubPushDevice> {
        self.subscribe_push_at(web_session_id, s, now_ms())
    }

    pub fn subscribe_push_at(
        &self,
        web_session_id: &str,
        s: &NewPushSubscription,
        now: i64,
    ) -> Result<HubPushDevice> {
        let endpoint = s.endpoint.trim();
        if endpoint.is_empty() || endpoint.len() > ENDPOINT_MAX {
            return Err(LedgerError::Conflict("endpoint de push inválido".into()));
        }
        let tx = self.conn.unchecked_transaction()?;
        let exists: bool = self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM push_subscription WHERE endpoint = ?1)",
            [endpoint],
            |r| r.get(0),
        )?;
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM push_subscription", [], |r| r.get(0))?;
        if !exists && count >= PUSH_SUBSCRIPTIONS_MAX {
            return Err(LedgerError::Conflict(format!(
                "limite de {PUSH_SUBSCRIPTIONS_MAX} dispositivos inscritos: remova algum antes"
            )));
        }
        let label = s
            .label
            .as_deref()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|l| clip(l, LABEL_MAX));
        let events = events_json(s.events.as_deref().unwrap_or(&PushEvent::ALL));
        self.conn.execute(
            "INSERT INTO push_subscription (web_session_id, endpoint, p256dh, auth, label, events,
                                            created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (endpoint) DO UPDATE SET
                web_session_id = excluded.web_session_id, p256dh = excluded.p256dh,
                auth = excluded.auth, label = COALESCE(excluded.label, label),
                events = CASE WHEN ?8 THEN excluded.events ELSE events END,
                failures = 0, disabled_at = NULL",
            params![
                web_session_id,
                endpoint,
                s.p256dh.trim(),
                s.auth.trim(),
                label,
                events,
                now,
                s.events.is_some()
            ],
        )?;
        let d = self.conn.query_row(
            &format!("SELECT {DEVICE_COLS} FROM push_subscription WHERE endpoint = ?1"),
            [endpoint],
            |r| row_to_device(r, Some(web_session_id)),
        )?;
        tx.commit()?;
        Ok(d)
    }

    /// Dispositivos inscritos, do mais novo para o mais antigo. `current` =
    /// a sessão web da conexão.
    pub fn list_push_devices(&self, current: Option<&str>) -> Result<Vec<HubPushDevice>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {DEVICE_COLS} FROM push_subscription ORDER BY created_at DESC, id DESC"
        ))?;
        let rows = stmt
            .query_map([], |r| row_to_device(r, current))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// Troca os eventos do dispositivo `id`; `None` = não existe.
    pub fn update_push_events(
        &self,
        id: i64,
        events: &[PushEvent],
        current: Option<&str>,
    ) -> Result<Option<HubPushDevice>> {
        let n = self.conn.execute(
            "UPDATE push_subscription SET events = ?2 WHERE id = ?1",
            params![id, events_json(events)],
        )?;
        if n == 0 {
            return Ok(None);
        }
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {DEVICE_COLS} FROM push_subscription WHERE id = ?1"),
                [id],
                |r| row_to_device(r, current),
            )
            .optional()?)
    }

    /// Remove a inscrição `id`. `false` se não existia.
    pub fn remove_push_subscription(&self, id: i64) -> Result<bool> {
        Ok(self
            .conn
            .execute("DELETE FROM push_subscription WHERE id = ?1", [id])?
            > 0)
    }

    /// Destino do dispositivo `id` (ativo ou não), para o `hub.push.test`.
    pub fn push_target(&self, id: i64) -> Result<Option<PushTarget>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, endpoint, p256dh, auth FROM push_subscription WHERE id = ?1",
                [id],
                row_to_target,
            )
            .optional()?)
    }

    /// Inscrições ativas que recebem `event`, de sessões web não revogadas.
    pub fn push_targets(&self, event: PushEvent) -> Result<Vec<PushTarget>> {
        let mut stmt = self.conn.prepare(
            "SELECT ps.id, ps.endpoint, ps.p256dh, ps.auth
             FROM push_subscription ps JOIN web_session ws ON ws.id = ps.web_session_id
             WHERE ps.disabled_at IS NULL AND ws.revoked_at IS NULL
               AND EXISTS (SELECT 1 FROM json_each(ps.events) WHERE value = ?1)
             ORDER BY ps.id",
        )?;
        let rows = stmt
            .query_map([event.as_str()], row_to_target)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }

    /// O serviço de push aceitou a entrega: zera as falhas.
    pub fn push_delivered(&self, id: i64, now: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE push_subscription SET last_ok_at = ?2, failures = 0 WHERE id = ?1",
            params![id, now],
        )?;
        Ok(())
    }

    /// Falha fora 404/410: conta e, com [`PUSH_FAILURES_MAX`] seguidas,
    /// desativa. Devolve se a inscrição ficou desativada agora.
    pub fn push_failed(&self, id: i64, now: i64) -> Result<bool> {
        self.conn.execute(
            "UPDATE push_subscription SET failures = failures + 1 WHERE id = ?1",
            [id],
        )?;
        Ok(self.conn.execute(
            "UPDATE push_subscription SET disabled_at = ?2
             WHERE id = ?1 AND disabled_at IS NULL AND failures >= ?3",
            params![id, now, PUSH_FAILURES_MAX],
        )? > 0)
    }

    /// Grava a chave como notificada; `false` se já estava.
    fn push_mark(&self, key: &str, now: i64) -> Result<bool> {
        Ok(self.conn.execute(
            "INSERT INTO push_sent (key, sent_at) VALUES (?1, ?2) ON CONFLICT (key) DO NOTHING",
            params![key, now],
        )? > 0)
    }

    /// Notificações novas desde a última varredura, já marcadas em
    /// `push_sent` (uma chave nunca sai duas vezes). Olha sessões, eventos
    /// e propostas de até `window_ms` atrás e ambientes sem sync há mais de
    /// `stale_ms` (`0` = desligado). Na primeira varredura do hub só marca
    /// o que já existia, sem notificar.
    ///
    /// Com canal reverso (`channels`), o `env_stale` do ambiente é o do
    /// canal, em tempo real: fechado há mais de [`CHANNEL_STALE_MS`]. Os
    /// outros seguem pelo último sync.
    pub fn push_collect(
        &self,
        window_ms: i64,
        stale_ms: i64,
        channels: &ChannelStates,
    ) -> Result<Vec<PushNotice>> {
        self.push_collect_with(now_ms(), window_ms, stale_ms, channels)
    }

    /// [`Hub::push_collect`] num instante dado, sem canal reverso.
    pub fn push_collect_at(
        &self,
        now: i64,
        window_ms: i64,
        stale_ms: i64,
    ) -> Result<Vec<PushNotice>> {
        self.push_collect_with(now, window_ms, stale_ms, &ChannelStates::new())
    }

    /// [`Hub::push_collect`] num instante dado.
    pub fn push_collect_with(
        &self,
        now: i64,
        window_ms: i64,
        stale_ms: i64,
        channels: &ChannelStates,
    ) -> Result<Vec<PushNotice>> {
        let tx = self.conn.unchecked_transaction()?;
        self.conn.execute(
            "DELETE FROM push_sent WHERE sent_at < ?1 AND key <> ?2",
            params![now - PUSH_SENT_KEEP_MS, BASELINE_KEY],
        )?;
        let baseline = self.push_mark(BASELINE_KEY, now)?;
        let since = now - window_ms;
        let mut all = Vec::new();

        // Sessões que saíram com erro.
        let mut stmt = self.conn.prepare(&format!(
            "{SESSION_JOIN}
             WHERE s.state = 'exited' AND s.exit_code IS NOT NULL AND s.exit_code <> 0
               AND s.ended_at >= ?1 AND p.deleted_at IS NULL
             ORDER BY s.ended_at, s.id"
        ))?;
        let rows = stmt
            .query_map([since], |r| row_to_session(r, 0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for s in rows {
            all.push(PushNotice {
                key: format!("session_error:{}", s.id),
                event: PushEvent::SessionError,
                title: "Sessão saiu com erro".into(),
                body: item_body(
                    &s.owner,
                    &s.repo,
                    s.number,
                    s.title.as_deref(),
                    s.gestor,
                    s.jira_key.as_deref(),
                ),
                url: session_url(&s),
                tag: format!("session:{}", s.id),
            });
        }

        // "Aguardando você" de sessão viva sem Remote Control: a com Remote
        // Control já recebe o push nativo do app Claude (spec §7.1).
        let mut stmt = self.conn.prepare(&format!(
            "SELECT e.uid, x.* FROM event e
             JOIN ({SESSION_JOIN} WHERE p.deleted_at IS NULL) x ON x.id = e.session_id
             JOIN session s ON s.id = e.session_id
             WHERE e.type IN ('needs_human', 'permission_request') AND e.ts >= ?1
               AND s.state IN ('starting', 'running', 'idle')
               AND (s.rc_session_id IS NULL OR trim(s.rc_session_id) = '')
             ORDER BY e.ts, e.id"
        ))?;
        let rows = stmt
            .query_map([since], |r| {
                Ok((r.get::<_, String>(0)?, row_to_session(r, 1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (uid, s) in rows {
            all.push(PushNotice {
                key: format!("needs_human:{}:{uid}", s.id),
                event: PushEvent::NeedsHuman,
                title: "Aguardando você".into(),
                body: item_body(
                    &s.owner,
                    &s.repo,
                    s.number,
                    s.title.as_deref(),
                    s.gestor,
                    s.jira_key.as_deref(),
                ),
                url: session_url(&s),
                tag: format!("session:{}", s.id),
            });
        }

        // Propostas novas do gestor.
        let mut stmt = self.conn.prepare(&format!(
            "SELECT pr.id, pr.kind, p.gh_owner, p.gh_repo, pr.gh_number, ic.title, {JIRA_KEY_COL}
             FROM proposal pr JOIN project p ON p.id = pr.project_id
             LEFT JOIN issue_cache ic ON ic.project_id = p.id AND ic.gh_number = pr.gh_number
             WHERE pr.state = 'pendente' AND pr.created_at >= ?1
             ORDER BY pr.created_at, pr.id"
        ))?;
        let rows = stmt
            .query_map([since], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<String>>(6)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, kind, owner, repo, n, title, jira_key) in rows {
            let what = if kind == "priority" {
                "prioridade"
            } else {
                "status"
            };
            all.push(PushNotice {
                key: format!("proposal_new:{id}"),
                event: PushEvent::ProposalNew,
                title: format!("Proposta nova do gestor ({what})"),
                body: item_body(
                    &owner,
                    &repo,
                    n,
                    title.as_deref(),
                    false,
                    jira_key.as_deref(),
                ),
                url: "/?go=hub:proposals".into(),
                tag: "proposals".into(),
            });
        }

        // Ambientes parados. Com canal reverso, vale o canal (fechado há mais
        // de `CHANNEL_STALE_MS`; a chave leva quando fechou); sem canal, o
        // último envio (a chave leva o envio). Nos dois, só repete depois que
        // o ambiente voltar e sumir de novo.
        if stale_ms > 0 {
            let mut stmt = self.conn.prepare(
                "SELECT e.env_id, e.name, e.last_seen_at FROM env e
                 WHERE EXISTS (SELECT 1 FROM env_token t WHERE t.env_id = e.env_id)
                 ORDER BY e.name",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<i64>>(2)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for (env_id, name, seen) in rows {
                let (key, body) = match channels.get(&env_id) {
                    Some(None) => continue,
                    Some(Some(closed)) => {
                        if now - closed < CHANNEL_STALE_MS {
                            continue;
                        }
                        let min = (now - closed) / 60_000;
                        (
                            format!("env_stale:{env_id}:canal:{closed}"),
                            format!("{name}: sem canal com o hub há {min} min"),
                        )
                    }
                    None => {
                        let Some(seen) = seen.filter(|s| *s < now - stale_ms) else {
                            continue;
                        };
                        let min = (now - seen) / 60_000;
                        (
                            format!("env_stale:{env_id}:{seen}"),
                            format!("{name}: sem enviar ao hub há {min} min"),
                        )
                    }
                };
                all.push(PushNotice {
                    key,
                    event: PushEvent::EnvStale,
                    title: "Ambiente fora do ar".into(),
                    body,
                    url: "/?go=hub:envs".into(),
                    tag: format!("env:{env_id}"),
                });
            }
        }
        drop(stmt);

        let mut out = Vec::new();
        for n in all {
            if self.push_mark(&n.key, now)? && !baseline {
                out.push(n);
            }
        }
        tx.commit()?;
        Ok(out)
    }
}

fn row_to_target(r: &Row) -> rusqlite::Result<PushTarget> {
    Ok(PushTarget {
        id: r.get(0)?,
        endpoint: r.get(1)?,
        p256dh: r.get(2)?,
        auth: r.get(3)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Ledger, NewProject, NewSession};
    use ct_core::{SessionState, Tool, WorkItemKind};

    const MIN: i64 = 60_000;
    const WINDOW: i64 = 3_600_000;

    fn sub(endpoint: &str) -> NewPushSubscription {
        NewPushSubscription {
            endpoint: endpoint.into(),
            p256dh: "chave".into(),
            auth: "segredo".into(),
            label: Some("Chrome · Android".into()),
            events: None,
        }
    }

    #[test]
    fn inscricao_por_endpoint_preferencias_e_remocao() {
        let hub = Hub::open_in_memory().unwrap();
        let s = hub.create_web_session(None, None, 3_600_000).unwrap();
        let d = hub
            .subscribe_push_at(&s.id, &sub("https://fcm.googleapis.com/fcm/send/abc"), 10)
            .unwrap();
        assert_eq!(d.events, PushEvent::ALL.to_vec());
        assert_eq!(d.service, "fcm.googleapis.com");
        assert!(d.current);
        // Mesmo endpoint: atualiza, não duplica; sem events mantém os de antes.
        let d2 = hub
            .update_push_events(d.id, &[PushEvent::NeedsHuman], None)
            .unwrap()
            .unwrap();
        assert_eq!(d2.events, [PushEvent::NeedsHuman]);
        assert!(!d2.current);
        let mut again = sub("https://fcm.googleapis.com/fcm/send/abc");
        again.label = None;
        let d3 = hub.subscribe_push_at(&s.id, &again, 20).unwrap();
        assert_eq!(d3.id, d.id);
        assert_eq!(d3.events, [PushEvent::NeedsHuman]);
        assert_eq!(d3.label.as_deref(), Some("Chrome · Android"));
        assert_eq!(hub.list_push_devices(None).unwrap().len(), 1);
        assert_eq!(hub.push_targets(PushEvent::NeedsHuman).unwrap().len(), 1);
        assert!(hub.push_targets(PushEvent::EnvStale).unwrap().is_empty());
        assert!(hub.update_push_events(999, &[], None).unwrap().is_none());
        assert!(hub.remove_push_subscription(d.id).unwrap());
        assert!(!hub.remove_push_subscription(d.id).unwrap());
    }

    #[test]
    fn falhas_seguidas_desativam_e_reinscrever_reativa() {
        let hub = Hub::open_in_memory().unwrap();
        let s = hub.create_web_session(None, None, 3_600_000).unwrap();
        let d = hub
            .subscribe_push_at(&s.id, &sub("https://p/x"), 1)
            .unwrap();
        for i in 1..PUSH_FAILURES_MAX {
            assert!(!hub.push_failed(d.id, i).unwrap());
        }
        hub.push_delivered(d.id, 50).unwrap();
        for i in 1..PUSH_FAILURES_MAX {
            assert!(!hub.push_failed(d.id, i).unwrap(), "zerou na entrega");
        }
        assert!(hub.push_failed(d.id, 99).unwrap());
        assert!(hub
            .push_targets(PushEvent::SessionError)
            .unwrap()
            .is_empty());
        let d = hub
            .subscribe_push_at(&s.id, &sub("https://p/x"), 2)
            .unwrap();
        assert_eq!((d.failures, d.disabled_at), (0, None));
        assert_eq!(hub.push_targets(PushEvent::SessionError).unwrap().len(), 1);
    }

    #[test]
    fn revogar_sessao_web_apaga_as_inscricoes_dela() {
        let hub = Hub::open_in_memory().unwrap();
        let a = hub.create_web_session(None, None, 3_600_000).unwrap();
        let b = hub.create_web_session(None, None, 3_600_000).unwrap();
        hub.subscribe_push_at(&a.id, &sub("https://p/a"), 1)
            .unwrap();
        hub.subscribe_push_at(&b.id, &sub("https://p/b"), 1)
            .unwrap();
        assert!(hub.revoke_web_session(&a.id).unwrap());
        let left = hub.push_targets(PushEvent::SessionError).unwrap();
        assert_eq!(
            left.iter().map(|t| t.endpoint.as_str()).collect::<Vec<_>>(),
            ["https://p/b"]
        );
        assert_eq!(hub.list_push_devices(None).unwrap().len(), 1);
        assert_eq!(hub.revoke_all_web_sessions().unwrap(), 1);
        assert!(hub.list_push_devices(None).unwrap().is_empty());
    }

    #[test]
    fn limite_de_dispositivos() {
        let hub = Hub::open_in_memory().unwrap();
        let s = hub.create_web_session(None, None, 3_600_000).unwrap();
        for i in 0..PUSH_SUBSCRIPTIONS_MAX {
            hub.subscribe_push_at(&s.id, &sub(&format!("https://p/{i}")), 1)
                .unwrap();
        }
        assert!(hub
            .subscribe_push_at(&s.id, &sub("https://p/novo"), 1)
            .is_err());
        // Reinscrever um existente segue valendo.
        assert!(hub.subscribe_push_at(&s.id, &sub("https://p/0"), 2).is_ok());
    }

    /// Hub com o item #7 do projeto demo e uma sessão `s` do ambiente "wsl".
    fn hub_com_sessao(rc: Option<&str>) -> (Hub, Ledger, String) {
        let hub = Hub::open_in_memory().unwrap();
        hub.add_project(&crate::NewHubProject {
            owner: "deep-ion-ai".into(),
            repo: "demo".into(),
            name: "demo".into(),
            base_branch: "develop".into(),
            protected_branches: vec![],
        })
        .unwrap();
        hub.add_legacy_env_token("wsl").unwrap();
        let l = Ledger::open_in_memory().unwrap();
        let p = l
            .add_project(&NewProject {
                name: "demo".into(),
                gh_owner: "deep-ion-ai".into(),
                gh_repo: "demo".into(),
                repo_path: "/x".into(),
                base_branch: "develop".into(),
                protected_branches: vec![],
            })
            .unwrap();
        let w = l.ensure_work_item(p.id, 7, WorkItemKind::Issue).unwrap();
        let s = l
            .create_session(&NewSession {
                work_item_id: w.id,
                tool: Tool::Claude,
                cwd: "/x".into(),
                launch_cmd: vec![],
                model: None,
                mode: Default::default(),
            })
            .unwrap();
        if let Some(rc) = rc {
            l.set_session_rc_id(&s.id, rc).unwrap();
        }
        l.transition_session(&s.id, SessionState::Running, None)
            .unwrap();
        hub.bind_env_token("wsl", l.env_id()).unwrap();
        hub.ingest(l.env_id(), "wsl", &l.outbox_pending(1000).unwrap())
            .unwrap();
        let pid: i64 = hub
            .conn
            .query_row(
                "SELECT id FROM project WHERE key = 'deep-ion-ai/demo'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        hub.conn
            .execute(
                "INSERT INTO issue_cache (project_id, gh_number, title, state, url, updated_at,
                                          synced_at)
                 VALUES (?1, 7, 'Web Push', 'open', 'https://x', 1, 1)",
                [pid],
            )
            .unwrap();
        (hub, l, s.id)
    }

    fn event(l: &Ledger, sid: &str, kind: &str) {
        let s = l.get_session(sid).unwrap();
        let w = l.get_work_item(s.work_item_id).unwrap();
        l.append_event(crate::NewEvent {
            project_id: w.project_id,
            work_item_id: Some(w.id),
            session_id: Some(sid),
            kind,
            payload: serde_json::json!({ "message": "segredo" }),
        })
        .unwrap();
    }

    fn ingest(hub: &Hub, l: &Ledger) {
        hub.ingest(l.env_id(), "wsl", &l.outbox_pending(1000).unwrap())
            .unwrap();
    }

    #[test]
    fn aguardando_voce_so_sem_remote_control_e_deduplicado() {
        let (hub, l, sid) = hub_com_sessao(None);
        let now = now_ms();
        assert!(
            hub.push_collect_at(now, WINDOW, 0).unwrap().is_empty(),
            "base"
        );
        event(&l, &sid, "needs_human");
        ingest(&hub, &l);
        let got = hub.push_collect_at(now_ms(), WINDOW, 0).unwrap();
        assert_eq!(got.len(), 1, "{got:?}");
        let n = &got[0];
        assert_eq!(n.event, PushEvent::NeedsHuman);
        assert_eq!(n.title, "Aguardando você");
        assert_eq!(n.body, "deep-ion-ai/demo #7 · Web Push");
        assert_eq!(n.url, "/?go=item&project=deep-ion-ai/demo&n=7");
        assert!(!serde_json::to_string(n).unwrap().contains("segredo"));
        assert!(n.key.starts_with(&format!("needs_human:{sid}:")));
        // Deduplicado: a mesma varredura não repete.
        assert!(hub.push_collect_at(now_ms(), WINDOW, 0).unwrap().is_empty());

        // Com Remote Control, nada.
        let (hub, l, sid) = hub_com_sessao(Some("session_01abc"));
        hub.push_collect_at(now_ms(), WINDOW, 0).unwrap();
        event(&l, &sid, "needs_human");
        event(&l, &sid, "permission_request");
        ingest(&hub, &l);
        assert!(hub.push_collect_at(now_ms(), WINDOW, 0).unwrap().is_empty());
    }

    /// Projeto Jira (épico #353): o corpo leva a chave da issue no lugar de
    /// `owner/repo #n`; o deep link segue pelo `owner/repo` do hub.
    #[test]
    fn corpo_com_a_chave_do_jira() {
        let (hub, l, sid) = hub_com_sessao(None);
        hub.conn
            .execute(
                "UPDATE project SET tracker_kind = 'jira', forge_kind = 'gitlab',
                        tracker_config = '{\"url\":\"https://jira.x\",\"project_key\":\"PROJ\"}'
                 WHERE key = 'deep-ion-ai/demo'",
                [],
            )
            .unwrap();
        hub.push_collect_at(now_ms(), WINDOW, 0).unwrap();
        event(&l, &sid, "needs_human");
        ingest(&hub, &l);
        let got = hub.push_collect_at(now_ms(), WINDOW, 0).unwrap();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].body, "PROJ-7 · Web Push");
        assert_eq!(got[0].url, "/?go=item&project=deep-ion-ai/demo&n=7");
    }

    #[test]
    fn sessao_com_erro_proposta_e_ambiente_parado() {
        let (hub, l, sid) = hub_com_sessao(None);
        let t0 = now_ms();
        assert!(hub
            .push_collect_at(t0, WINDOW, 15 * MIN)
            .unwrap()
            .is_empty());
        l.transition_session(&sid, SessionState::Exited, Some(2))
            .unwrap();
        ingest(&hub, &l);
        let got = hub.push_collect_at(now_ms(), WINDOW, 15 * MIN).unwrap();
        assert_eq!(
            got.iter().map(|n| n.event).collect::<Vec<_>>(),
            [PushEvent::SessionError]
        );
        assert_eq!(got[0].key, format!("session_error:{sid}"));

        let p = hub
            .create_proposal(
                &crate::NewProposal {
                    owner: "deep-ion-ai".into(),
                    repo: "demo".into(),
                    number: 7,
                    kind: ct_core::ProposalKind::Priority,
                    proposed: serde_json::json!({ "priority": "P1" }),
                    justification: "urgente".into(),
                },
                l.env_id(),
                None,
                now_ms(),
            )
            .unwrap();
        let got = hub.push_collect_at(now_ms(), WINDOW, 15 * MIN).unwrap();
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].key, format!("proposal_new:{}", p.id));
        assert_eq!(got[0].url, "/?go=hub:proposals");

        // Ambiente sem sync há 20 min: um push só, até ele voltar.
        let later = now_ms() + 20 * MIN;
        let got = hub.push_collect_at(later, WINDOW, 15 * MIN).unwrap();
        assert_eq!(
            got.iter().map(|n| n.event).collect::<Vec<_>>(),
            [PushEvent::EnvStale]
        );
        assert_eq!(got[0].url, "/?go=hub:envs");
        assert!(
            got[0].body.starts_with("wsl: sem enviar ao hub há 2"),
            "{}",
            got[0].body
        );
        assert!(hub
            .push_collect_at(later + 10 * MIN, WINDOW, 15 * MIN)
            .unwrap()
            .is_empty());
        // Voltou (novo envio) e sumiu de novo: outro push.
        ingest(&hub, &l);
        let again = now_ms() + 40 * MIN;
        let got = hub.push_collect_at(again, 2 * WINDOW, 15 * MIN).unwrap();
        assert_eq!(
            got.iter().map(|n| n.event).collect::<Vec<_>>(),
            [PushEvent::EnvStale]
        );
        // Desligado (0): nunca.
        let (hub, _l, _) = hub_com_sessao(None);
        hub.push_collect_at(now_ms(), WINDOW, 0).unwrap();
        assert!(hub
            .push_collect_at(now_ms() + 600 * MIN, WINDOW, 0)
            .unwrap()
            .is_empty());
    }

    /// Fase 2 (spec §7.1): com canal reverso, o `env_stale` é o do canal, em
    /// tempo real (fechado há mais de 1 min), e não a idade do último sync.
    #[test]
    fn ambiente_parado_pelo_canal_em_tempo_real() {
        let (hub, l, _) = hub_com_sessao(None);
        let env = l.env_id().to_string();
        let t0 = now_ms();
        hub.push_collect_at(t0, WINDOW, 15 * MIN).unwrap();
        let ch = |v: Option<i64>| ChannelStates::from([(env.clone(), v)]);

        // Canal aberto: sem push, mesmo com o último sync velho.
        let later = t0 + 20 * MIN;
        assert!(hub
            .push_collect_with(later, WINDOW, 15 * MIN, &ch(None))
            .unwrap()
            .is_empty());
        // Fechado há 30 s: ainda não; há 2 min: um push, mesmo com sync recente.
        let closed = t0 + MIN;
        assert!(hub
            .push_collect_with(closed + 30_000, WINDOW, 15 * MIN, &ch(Some(closed)))
            .unwrap()
            .is_empty());
        ingest(&hub, &l);
        let got = hub
            .push_collect_with(closed + 2 * MIN, WINDOW, 15 * MIN, &ch(Some(closed)))
            .unwrap();
        assert_eq!(
            got.iter().map(|n| n.event).collect::<Vec<_>>(),
            [PushEvent::EnvStale]
        );
        assert_eq!(got[0].key, format!("env_stale:{env}:canal:{closed}"));
        assert!(
            got[0].body.starts_with("wsl: sem canal com o hub há 2 min"),
            "{}",
            got[0].body
        );
        assert!(hub
            .push_collect_with(closed + 5 * MIN, WINDOW, 15 * MIN, &ch(Some(closed)))
            .unwrap()
            .is_empty());
        // Voltou e caiu de novo: outro push.
        let again = closed + 10 * MIN;
        let got = hub
            .push_collect_with(again + 2 * MIN, WINDOW, 15 * MIN, &ch(Some(again)))
            .unwrap();
        assert_eq!(got.len(), 1, "{got:?}");
        // Desligado (0): nunca, nem pelo canal.
        assert!(hub
            .push_collect_with(again + 60 * MIN, WINDOW, 0, &ch(Some(again + MIN)))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn primeira_varredura_so_marca() {
        let (hub, l, sid) = hub_com_sessao(None);
        event(&l, &sid, "needs_human");
        ingest(&hub, &l);
        assert!(hub.push_collect_at(now_ms(), WINDOW, 0).unwrap().is_empty());
        assert!(hub.push_collect_at(now_ms(), WINDOW, 0).unwrap().is_empty());
    }

    #[test]
    fn deep_link_do_remote_control() {
        let s = SessionRow {
            id: "s".into(),
            owner: "o".into(),
            repo: "r".into(),
            number: 3,
            title: None,
            gestor: false,
            rc_session_id: Some("session_9".into()),
            live: true,
            jira_key: None,
        };
        assert_eq!(session_url(&s), "https://claude.ai/code/session_9");
        let ended = SessionRow { live: false, ..s };
        assert_eq!(session_url(&ended), "/?go=item&project=o/r&n=3");
        assert_eq!(clip(&"x".repeat(200), 10).chars().count(), 10);
        assert_eq!(
            service_of("https://web.push.apple.com/abc?x"),
            "web.push.apple.com"
        );
    }
}
