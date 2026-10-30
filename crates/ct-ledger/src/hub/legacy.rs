//! Adoção de um `hub.db` do #64 (o `ctowerd` com `hub = true`) pelo
//! `cthubd migrate` (spec do hub como plano de controle §11, issue #182).
//!
//! As migrações 6 a 8 do esquema já rodam no `Hub::open`. O que elas não
//! fazem, porque depende da instalação, fica aqui: promover os projetos a
//! origem (com `rev`), criar as atribuições a partir do que cada ambiente
//! relatou (`project_location` não removida) e o token de sync do `ctowerd`
//! local, que enviava em processo sem token.

use super::assignments::normalize_repo_path;
use super::{Hub, IssuedEnvToken};
use crate::{LedgerError, Result};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::path::Path;

/// Chave de `hub_meta` com o instante (ms) da adoção. Marca o `hub.db` como
/// migrado: a adoção não roda duas vezes.
const MIGRATED_AT: &str = "migrated_at";

/// Resultado de [`Hub::adopt_legacy`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyAdoption {
    /// Projetos promovidos a origem (ganharam `rev`).
    pub projects: usize,
    /// Atribuições criadas a partir de `project_location`.
    pub assignments: usize,
    /// `config_rev` depois da adoção.
    pub config_rev: i64,
}

impl Hub {
    /// Instante da adoção (`cthubd migrate`), se o `hub.db` já foi migrado.
    pub fn migrated_at(&self) -> Result<Option<i64>> {
        meta(&self.conn, MIGRATED_AT)
    }

    /// Como [`Hub::migrated_at`], direto no arquivo e só leitura (sem
    /// aplicar migrações). Arquivo sem `hub_meta`: `None`.
    pub fn migrated_at_path(path: &Path) -> Result<Option<i64>> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(crate::BUSY_TIMEOUT)?;
        match meta(&conn, MIGRATED_AT) {
            Err(LedgerError::Sqlite(rusqlite::Error::SqliteFailure(_, Some(m))))
                if m.contains("no such table") =>
            {
                Ok(None)
            }
            r => r,
        }
    }

    /// Promove os projetos do hub antigo a origem e cria as atribuições, numa
    /// transação: cada projeto vivo ainda sem `rev` ganha um `config_rev`
    /// novo, e cada `project_location` não removida vira uma atribuição
    /// (com o `repo_path` relatado) com o seu. Grava `migrated_at`; num
    /// `hub.db` já migrado, recusa.
    pub fn adopt_legacy(&self, now_ms: i64) -> Result<LegacyAdoption> {
        let tx = self.conn.unchecked_transaction()?;
        if self.migrated_at()?.is_some() {
            return Err(LedgerError::Conflict("hub.db já migrado".into()));
        }
        // Impressão digital do hub antigo no momento da cópia: uma retomada
        // confere se ele recebeu algo depois (§11, voltar atrás).
        for (k, v) in fingerprint(&self.conn)? {
            self.conn.execute(
                "INSERT INTO hub_meta (key, value) VALUES (?1, ?2)
                 ON CONFLICT (key) DO UPDATE SET value = excluded.value",
                params![format!("{SRC_PREFIX}{k}"), v],
            )?;
        }
        let ids: Vec<i64> = {
            let mut stmt = self.conn.prepare(
                "SELECT id FROM project WHERE deleted_at IS NULL AND rev = 0 AND kind = 'github'
                 ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for id in &ids {
            let rev = self.bump_config_rev()?;
            self.conn.execute(
                "UPDATE project SET rev = ?2 WHERE id = ?1",
                params![id, rev],
            )?;
        }
        let locations: Vec<(i64, String, String)> = {
            let mut stmt = self.conn.prepare(
                "SELECT pl.project_id, pl.env_id, pl.repo_path FROM project_location pl
                 JOIN project p ON p.id = pl.project_id
                 WHERE pl.removed = 0 AND p.deleted_at IS NULL
                   AND NOT EXISTS (SELECT 1 FROM assignment a
                                   WHERE a.project_id = pl.project_id AND a.env_id = pl.env_id)
                 ORDER BY pl.project_id, pl.env_id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        for (project_id, env_id, path) in &locations {
            // Caminho relatado fora do formato aceito: o padrão do ambiente.
            let path = normalize_repo_path(Some(path)).unwrap_or_default();
            let rev = self.bump_config_rev()?;
            self.conn.execute(
                "INSERT INTO assignment (project_id, env_id, repo_path, created_at, rev)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![project_id, env_id, path, now_ms, rev],
            )?;
        }
        self.conn.execute(
            "INSERT INTO hub_meta (key, value) VALUES (?1, ?2)",
            params![MIGRATED_AT, now_ms],
        )?;
        let config_rev = self.config_rev()?;
        tx.commit()?;
        Ok(LegacyAdoption {
            projects: ids.len(),
            assignments: locations.len(),
            config_rev,
        })
    }

    /// Nome do token de sync preso a `env_id`, se houver.
    pub fn env_token_name_of(&self, env_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT name FROM env_token WHERE env_id = ?1",
                [env_id],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// `env_id` preso ao token de `name`: `None` = não há token com esse
    /// nome; `Some(None)` = token ainda sem `env_id`.
    pub fn env_token_env_id(&self, name: &str) -> Result<Option<Option<String>>> {
        Ok(self
            .conn
            .query_row(
                "SELECT env_id FROM env_token WHERE name = ?1",
                [name.trim()],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Gera (ou troca) o token de sync de `name` já preso a `env_id`, numa
    /// transação. É o token do `ctowerd` que era o hub: o `env_id` dele já
    /// está no histórico, e prender agora evita outro ambiente usá-lo.
    pub fn add_bound_env_token(&self, name: &str, env_id: &str) -> Result<IssuedEnvToken> {
        self.issue_env_token_for(name, Some(env_id))
    }
}

/// Prefixo, em `hub_meta`, da impressão digital do hub antigo.
const SRC_PREFIX: &str = "migrated_src:";

/// O que mudaria no `hub.db` antigo se ele recebesse algo: envios dos
/// ambientes (`env.last_seq`), eventos, decisões (e revisões), sessões,
/// projetos, localizações e tokens. Consultas só do esquema do #64.
const FINGERPRINT: [(&str, &str); 12] = [
    ("env_count", "SELECT count(*) FROM env"),
    ("env_seq", "SELECT total(last_seq) FROM env"),
    ("event_count", "SELECT count(*) FROM event"),
    ("event_max", "SELECT coalesce(max(id), 0) FROM event"),
    ("decision_count", "SELECT count(*) FROM decision"),
    ("decision_max", "SELECT coalesce(max(id), 0) FROM decision"),
    (
        "decision_reviewed",
        "SELECT count(reviewed_at) FROM decision",
    ),
    ("session_count", "SELECT count(*) FROM session"),
    (
        "session_last",
        "SELECT coalesce(max(last_event_at), 0) FROM session",
    ),
    // Sem o sentinela do gestor, que a migração 14 cria só na cópia.
    (
        "project_count",
        "SELECT count(*) FROM project WHERE key <> '_ctower/gestor'",
    ),
    (
        "location_hlc",
        "SELECT coalesce(max(hlc), 0) + count(*) FROM project_location",
    ),
    (
        "token_last",
        "SELECT count(*) * 1000003 + coalesce(max(created_at), 0) FROM env_token",
    ),
];

fn fingerprint(conn: &Connection) -> Result<Vec<(&'static str, i64)>> {
    FINGERPRINT
        .iter()
        .map(|(k, q)| {
            let v: f64 =
                conn.query_row(&format!("SELECT CAST(({q}) AS REAL)"), [], |r| r.get(0))?;
            Ok((*k, v as i64))
        })
        .collect()
}

impl Hub {
    /// O `hub.db` antigo em `source` está igual ao que foi copiado na
    /// migração deste `hub.db`? `Ok(false)` também quando este `hub.db` não
    /// guardou a impressão digital. Abre `source` só para leitura.
    pub fn legacy_source_unchanged(&self, source: &Path) -> Result<bool> {
        let conn = Connection::open_with_flags(source, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.busy_timeout(crate::BUSY_TIMEOUT)?;
        for (k, v) in fingerprint(&conn)? {
            if meta(&self.conn, &format!("{SRC_PREFIX}{k}"))? != Some(v) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

fn meta(conn: &Connection, key: &str) -> Result<Option<i64>> {
    Ok(conn
        .query_row("SELECT value FROM hub_meta WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::HUB_MIGRATIONS;
    use crate::{AssignmentFilter, AssignmentState};

    /// `hub.db` no esquema do #64 (migrações 1 a 5), com dois ambientes:
    /// `local` relatou `demo` e `outro`; `remoto` relatou `demo` e removeu
    /// `velho`.
    fn hub_antigo(path: &Path) {
        let conn = Connection::open(path).unwrap();
        for m in &HUB_MIGRATIONS[..5] {
            conn.execute_batch(m).unwrap();
        }
        conn.execute_batch(
            "PRAGMA user_version = 5;
             INSERT INTO env (env_id, name, created_at) VALUES ('local', 'mac', 1), ('remoto', 'nb', 1);
             INSERT INTO project (id, key, gh_owner, gh_repo, name, created_at) VALUES
               (1, 'o/demo', 'o', 'demo', 'demo', 1),
               (2, 'o/outro', 'o', 'outro', 'outro', 1),
               (3, 'o/velho', 'o', 'velho', 'velho', 1);
             INSERT INTO project_location (project_id, env_id, repo_path, removed, hlc) VALUES
               (1, 'local', '/u/demo', 0, 1), (2, 'local', '/u/outro', 0, 1),
               (1, 'remoto', '/b/demo', 0, 1), (3, 'remoto', '/b/velho', 1, 1),
               (2, 'remoto', 'relativo', 0, 1);
             INSERT INTO env_token (name, token_hash, env_id, created_at)
               VALUES ('nb', 'h', 'remoto', 1);",
        )
        .unwrap();
    }

    #[test]
    fn adocao_promove_projetos_e_cria_atribuicoes_uma_vez() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("hub.db");
        hub_antigo(&db);
        assert_eq!(Hub::migrated_at_path(&db).unwrap(), None);
        let hub = Hub::open(&db).unwrap();
        assert_eq!(hub.config_rev().unwrap(), 0);
        let r = hub.adopt_legacy(42).unwrap();
        assert_eq!((r.projects, r.assignments, r.config_rev), (3, 4, 7));
        assert_eq!(hub.migrated_at().unwrap(), Some(42));
        assert_eq!(Hub::migrated_at_path(&db).unwrap(), Some(42));
        // Cada linha tem o seu rev, todos distintos e <= config_rev.
        let revs: Vec<i64> = ["demo", "outro", "velho"]
            .iter()
            .map(|r| hub.project_rev("o", r).unwrap().unwrap())
            .collect();
        assert_eq!(revs, vec![1, 2, 3]);
        let mut a: Vec<_> = hub
            .list_assignments(&AssignmentFilter::default())
            .unwrap()
            .into_iter()
            .map(|a| (a.gh_repo, a.env_id, a.repo_path, a.rev, a.state))
            .collect();
        a.sort_by(|x, y| (&x.0, &x.1).cmp(&(&y.0, &y.1)));
        assert_eq!(
            a,
            vec![
                (
                    "demo".into(),
                    "local".into(),
                    "/u/demo".into(),
                    4,
                    AssignmentState::Pending
                ),
                (
                    "demo".into(),
                    "remoto".into(),
                    "/b/demo".into(),
                    5,
                    AssignmentState::Pending
                ),
                (
                    "outro".into(),
                    "local".into(),
                    "/u/outro".into(),
                    6,
                    AssignmentState::Pending
                ),
                // Caminho relatado inválido: o padrão do ambiente.
                (
                    "outro".into(),
                    "remoto".into(),
                    "".into(),
                    7,
                    AssignmentState::Pending
                ),
            ]
        );
        // O hub antigo (a origem da cópia) está igual; depois de receber
        // algo, não.
        let src = d.path().join("src.db");
        hub_antigo(&src);
        assert!(hub.legacy_source_unchanged(&src).unwrap());
        let c = Connection::open(&src).unwrap();
        c.execute(
            "UPDATE env SET last_seq = last_seq + 1 WHERE env_id = 'remoto'",
            [],
        )
        .unwrap();
        assert!(!hub.legacy_source_unchanged(&src).unwrap());
        c.execute(
            "UPDATE env SET last_seq = last_seq - 1 WHERE env_id = 'remoto'",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO env_token (name, token_hash, created_at) VALUES ('novo', 'h2', 2)",
            [],
        )
        .unwrap();
        assert!(!hub.legacy_source_unchanged(&src).unwrap());
        // Uma vez só.
        let e = hub.adopt_legacy(43).unwrap_err();
        assert!(e.to_string().contains("já migrado"), "{e}");
        assert_eq!(hub.config_rev().unwrap(), 7);
        // O pull do ambiente local traz os dois atribuídos.
        let c = hub.env_config("local", None, None).unwrap();
        let mut ps: Vec<_> = c.projects.unwrap().iter().map(|p| p.repo.clone()).collect();
        ps.sort();
        assert_eq!(ps, vec!["demo", "outro"]);
    }

    #[test]
    fn token_preso_ao_env_id_do_hub_antigo() {
        let d = tempfile::tempdir().unwrap();
        let db = d.path().join("hub.db");
        hub_antigo(&db);
        let hub = Hub::open(&db).unwrap();
        assert_eq!(
            hub.env_token_name_of("remoto").unwrap().as_deref(),
            Some("nb")
        );
        assert_eq!(hub.env_token_name_of("local").unwrap(), None);
        assert_eq!(
            hub.env_token_env_id("nb").unwrap(),
            Some(Some("remoto".into()))
        );
        assert_eq!(hub.env_token_env_id("mac").unwrap(), None);
        let t = hub.add_bound_env_token("mac", "local").unwrap().token;
        let auth = hub.authenticate(&t).unwrap().unwrap();
        assert_eq!(
            (auth.name.as_str(), auth.env_id.as_deref()),
            ("mac", Some("local"))
        );
        // Trocar o token mantém o env_id.
        let t2 = hub.add_bound_env_token("mac", "local").unwrap().token;
        assert!(hub.authenticate(&t).unwrap().is_none());
        assert_eq!(
            hub.authenticate(&t2).unwrap().unwrap().env_id.as_deref(),
            Some("local")
        );
        // O env_id de outro token: recusa sem mudar nada.
        assert!(hub.add_bound_env_token("outro-nome", "remoto").is_err());
        assert_eq!(hub.env_token_env_id("outro-nome").unwrap(), None);
    }
}
